//! `GET /v1/chunks/{prefix}/{hash}` — PROTO-04 dedup query.
//!
//! **Ships as unconditional 404** per CONTEXT D-16 + PITFALL P3 + RESEARCH
//! §2.4. Xet-core treats `404` on this endpoint as "no global dedup
//! available, re-upload the chunk" — which is the legal degraded-mode
//! response for the whole of v1. Implementing real dedup requires a reverse
//! chunk index + shard response synthesis (`xet_core_structures::metadata_shard`
//! serialization, flagged OQ-G), deferred post-v1.
//!
//! Auth is still enforced (the extractor rejects missing/invalid bearers and
//! scope mismatches with 401/403 respectively) so instrumentation can cleanly
//! distinguish them from the 404. See threat model T-02-05-05.
//!
//! Routing (from Plan 02-05 Task 4):
//!   `GET /v1/chunks/{prefix}/{hash}` — **only path**. DO NOT register
//!   `GET /chunks/...` without `/v1`; xet-core's `remote_client.rs::query_dedup_api`
//!   calls `{endpoint}/v1/chunks/{key}` exclusively.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use serde_json::{Value, json};

use crate::auth::{AuthScoped, AuthStateRef};
use crate::errors::AppError;
use crate::scopes::SCOPE_DOWNLOAD;

/// Always returns `404 {"error":"not_found"}` with zero DB / Sia I/O.
///
/// Auth extractor runs first so missing/invalid bearers → 401 and scope
/// mismatch → 403 are still distinguishable. Ops visibility (e.g. a
/// `siahub_cas_dedup_query_total` Prometheus counter) lives inside
/// `tracing::debug!` here; Plan 02-09 may promote it to a real counter when
/// wiring metrics.
pub async fn query_dedup_shard<S>(
    State(_st): State<S>,
    AuthScoped(_ctx): AuthScoped<{ SCOPE_DOWNLOAD }>,
    Path((_prefix, _hash)): Path<(String, String)>,
) -> Result<(StatusCode, Json<Value>), AppError>
where
    S: AuthStateRef,
{
    tracing::debug!("dedup query (stub 404) — PROTO-04 D-16");
    Ok((StatusCode::NOT_FOUND, Json(json!({"error": "not_found"}))))
}
