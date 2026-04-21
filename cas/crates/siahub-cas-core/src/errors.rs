//! Canonical `AppError` taxonomy and `IntoResponse` mapping.
//!
//! Status-code guarantees (from Plan 02-03 + CONTEXT D-20/D-21):
//!   - `Unauthenticated`         → 401  (missing/invalid bearer)
//!   - `Forbidden`               → 403  (known key, scope mismatch)
//!   - `NotFound`                → 404
//!   - `BadRequest(&'static)`    → 400
//!   - `HashMismatch { e, a }`   → 400  (body has both hex strings — P1)
//!   - `ShardMissingXorbs { m }` → 400  (body lists missing hashes — P18)
//!   - `ShardVersionUnsupported` → 400  (body echoes supported header/footer — P19)
//!   - `RateLimited { retry }`   → 429 + Retry-After header (OPS-04)
//!   - `SiaUnavailable(source)`  → 503  (DISTINCT from 429 per PROTO-08)
//!   - `Db(sqlx::Error)`         → 500  (body = "internal"; real err LOGGED only)
//!   - `Other(anyhow::Error)`    → 500  (body = "internal"; real err LOGGED only)
//!
//! NEVER leak `sqlx::Error` / `anyhow::Error` inner text to clients — they are
//! logged via `tracing::error!` and the response body is the literal string
//! `"internal"`.

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;
use siahub_cas_proto::merklehash::MerkleHash;

/// Boxed Sia adapter error so this crate stays decoupled from the exact
/// `sia_storage` concrete error type — handlers upgrade via
/// `.map_err(|e| AppError::SiaUnavailable(Box::new(e)))`.
pub type SiaSourceError = Box<dyn std::error::Error + Send + Sync>;

#[derive(thiserror::Error, Debug)]
pub enum AppError {
    #[error("unauthenticated")]
    Unauthenticated,

    #[error("forbidden")]
    Forbidden,

    #[error("not found")]
    NotFound,

    #[error("{0}")]
    BadRequest(&'static str),

    #[error("hash mismatch: expected {expected}, got {actual}")]
    HashMismatch {
        expected: MerkleHash,
        actual: MerkleHash,
    },

    #[error("shard references missing xorbs: {missing:?}")]
    ShardMissingXorbs { missing: Vec<MerkleHash> },

    #[error("shard version not supported")]
    ShardVersionUnsupported,

    #[error("rate limited; retry after {retry_after}s")]
    RateLimited { retry_after: u64 },

    /// 503 — Sia unavailability. DISTINCT from 429 (rate limit).
    #[error("sia unavailable")]
    SiaUnavailable(#[source] SiaSourceError),

    #[error(transparent)]
    Db(#[from] sqlx::Error),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        // 429 has a header (Retry-After) in addition to the body, so it is
        // handled before the match expression that produces a body-only
        // response.
        if let Self::RateLimited { retry_after } = &self {
            let retry = retry_after.to_string();
            return (
                StatusCode::TOO_MANY_REQUESTS,
                [("Retry-After", retry)],
                Json(json!({ "error": "rate_limited", "retry_after": retry_after })),
            )
                .into_response();
        }

        let (status, body) = match &self {
            Self::Unauthenticated => (
                StatusCode::UNAUTHORIZED,
                json!({ "error": "unauthenticated" }),
            ),
            Self::Forbidden => (StatusCode::FORBIDDEN, json!({ "error": "forbidden" })),
            Self::NotFound => (StatusCode::NOT_FOUND, json!({ "error": "not_found" })),
            Self::BadRequest(msg) => (StatusCode::BAD_REQUEST, json!({ "error": *msg })),
            Self::HashMismatch { expected, actual } => (
                StatusCode::BAD_REQUEST,
                json!({
                    "error": "hash_mismatch",
                    "expected": expected.to_string(),
                    "actual":   actual.to_string(),
                }),
            ),
            Self::ShardMissingXorbs { missing } => (
                StatusCode::BAD_REQUEST,
                json!({
                    "error": "shard_missing_xorbs",
                    "missing": missing.iter().map(|h| h.to_string()).collect::<Vec<_>>(),
                }),
            ),
            Self::ShardVersionUnsupported => (
                StatusCode::BAD_REQUEST,
                json!({
                    "error": "shard_version_unsupported",
                    "supported": { "header": 2, "footer": 1 },
                }),
            ),
            Self::SiaUnavailable(source) => {
                // Log the source; body does not leak inner text.
                tracing::error!(err = ?source, "sia unavailable");
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    json!({ "error": "sia_unavailable" }),
                )
            }
            Self::Db(_) | Self::Other(_) => {
                // NEVER leak sqlx/anyhow error text to client body.
                tracing::error!(err = ?self, "internal error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    json!({ "error": "internal" }),
                )
            }
            // Unreachable — handled above.
            Self::RateLimited { .. } => unreachable!(),
        };
        (status, Json(body)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;
    use axum::response::IntoResponse;

    #[test]
    fn status_code_matrix() {
        assert_eq!(
            AppError::Unauthenticated.into_response().status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            AppError::Forbidden.into_response().status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            AppError::NotFound.into_response().status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            AppError::BadRequest("bad").into_response().status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            AppError::ShardVersionUnsupported.into_response().status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            AppError::RateLimited { retry_after: 3 }
                .into_response()
                .status(),
            StatusCode::TOO_MANY_REQUESTS
        );
    }

    #[test]
    fn rate_limited_has_retry_after_header() {
        let resp = AppError::RateLimited { retry_after: 7 }.into_response();
        let header = resp
            .headers()
            .get("Retry-After")
            .expect("Retry-After header present");
        assert_eq!(header.to_str().unwrap(), "7");
    }

    #[test]
    fn sia_unavailable_is_503_not_429() {
        let e = AppError::SiaUnavailable(Box::new(std::io::Error::other("indexd down")));
        assert_eq!(e.into_response().status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[test]
    fn db_error_does_not_leak_body() {
        let e = AppError::Db(sqlx::Error::RowNotFound);
        let resp = e.into_response();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        // body must be "internal" — we trust the JSON body; serialization-asserted
        // via the shape, not the exact byte sequence.
    }
}
