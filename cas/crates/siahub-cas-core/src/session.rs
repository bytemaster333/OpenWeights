//! Session cookie middleware + primitives for Phase 4 console admin routes.
//!
//! Plan 04-01 (Phase 2.1 amendment — D-41). Locks the `siahub_session`
//! cookie semantics (D-50 + D-54) and provides the `Session` extractor used
//! by every `/admin/*` handler + `POST /auth/logout`.
//!
//! Cookie spec (authoritative — D-50):
//!   * Name: `siahub_session`
//!   * Value: UUID v4 (opaque to client)
//!   * Flags: `HttpOnly; Secure; SameSite=Lax; Path=/`
//!   * Max-Age / Expires: 7 days, rolling (refreshed on every authenticated
//!     request via `touch_session`).
//!
//! Threat model:
//!   * `HttpOnly` blocks JS access — no XSS-driven theft.
//!   * `Secure` + TLS pins cookie to HTTPS in production (Caddy apex).
//!   * `SameSite=Lax` blocks cross-site CSRF for mutating routes (POST /
//!     DELETE). Combined with the OAuth `state` nonce, the auth round-trip
//!     is CSRF-safe.
//!   * Cookie value is ONLY ever a UUID — no PII, no user-id, no scope.
//!
//! DB columns read/written:
//!   * `sessions(session_id, user_id, created_at, expires_at, revoked_at,
//!     last_seen_at)` — `last_seen_at` added by migration
//!     `0006_sessions_touch.sql`.
//!   * `users(id, github_login, email, avatar_url, is_admin)` —
//!     `avatar_url` + `is_admin` added by the same migration.

use std::future::Future;

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use http::header::COOKIE;
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::AuthStateRef;
use crate::errors::AppError;

/// Session cookie name — locked by D-54. Console code must NEVER read / write
/// this cookie directly; only CAS mints it on `/auth/github/callback` and
/// clears it on `/auth/logout`.
pub const SESSION_COOKIE_NAME: &str = "siahub_session";

/// Session lifetime — 7 days rolling per D-50. Expiry is refreshed on every
/// authenticated hit via `touch_session`.
pub const SESSION_TTL_DAYS: i64 = 7;

/// De-serialized `users` row joined with the session row. Returned by the
/// `Session` extractor; handlers consume this directly.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionUser {
    pub id: i64,
    pub login: String,
    pub email: Option<String>,
    pub avatar_url: Option<String>,
    pub is_admin: bool,
}

/// Minted session metadata returned by `mint_session` so the OAuth callback
/// handler can build the `Set-Cookie` header in one place.
#[derive(Debug, Clone)]
pub struct MintedSession {
    pub session_id: Uuid,
    pub expires_at: DateTime<Utc>,
}

/// Extractor placed in a handler signature like:
///
/// ```ignore
/// async fn get_me(Session(user): Session) -> Json<MeResponse> { ... }
/// ```
///
/// Parsing failures (missing cookie, unknown session, expired, revoked) all
/// collapse to 401 per D-50 — the console's `api.ts` then redirects to
/// `/login`. No 403 is ever emitted by the extractor itself; admin-gated
/// handlers check `session.user.is_admin` in their own body.
#[derive(Debug, Clone)]
pub struct Session(pub SessionUser);

// ---------------------------------------------------------------------------
// Cookie header helpers
// ---------------------------------------------------------------------------

/// Extract `siahub_session=<uuid>` from a raw `Cookie:` header value. Tolerant
/// to multiple cookies (`; ` separated) and surrounding whitespace.
///
/// Returns `None` if the cookie is absent OR the value does not parse as a
/// UUID. Both outcomes produce a 401 at the extractor boundary — the client
/// observationally cannot distinguish them.
pub fn parse_session_cookie(cookie_header: &str) -> Option<Uuid> {
    for part in cookie_header.split(';') {
        let part = part.trim();
        if let Some(value) = part.strip_prefix(SESSION_COOKIE_NAME)
            && let Some(v) = value.strip_prefix('=')
        {
            return Uuid::parse_str(v.trim()).ok();
        }
    }
    None
}

/// Build the `Set-Cookie` header value for a minted session.
///
/// Header shape (exact text; asserted in unit tests):
/// ```text
/// siahub_session=<uuid>; HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age=604800
/// ```
///
/// `Max-Age=604800` is preferred over `Expires=` to keep the header robust
/// against client-clock skew (RFC 6265 §4.1.2.2).
pub fn session_cookie_header(session_id: Uuid) -> String {
    let max_age_secs = ChronoDuration::days(SESSION_TTL_DAYS).num_seconds();
    format!(
        "{}={}; HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age={}",
        SESSION_COOKIE_NAME, session_id, max_age_secs
    )
}

/// Build the `Set-Cookie` header value that clears the session cookie on
/// logout. `Max-Age=0` signals immediate deletion per RFC 6265 §5.2.2.
pub fn clear_session_cookie_header() -> String {
    format!(
        "{}=; HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age=0",
        SESSION_COOKIE_NAME
    )
}

// ---------------------------------------------------------------------------
// DB ops
// ---------------------------------------------------------------------------

/// Insert a fresh session row and return `(session_id, expires_at)`.
///
/// `session_id` is UUID v4 from `uuid::Uuid::new_v4` (CSPRNG-backed via
/// `getrandom`). `expires_at = NOW() + 7 days` per D-50.
pub async fn mint_session(db: &PgPool, user_id: i64) -> Result<MintedSession, sqlx::Error> {
    let session_id = Uuid::new_v4();
    let row: (DateTime<Utc>,) = sqlx::query_as(
        "INSERT INTO sessions (session_id, user_id, expires_at, last_seen_at) \
         VALUES ($1, $2, NOW() + ($3 || ' days')::interval, NOW()) \
         RETURNING expires_at",
    )
    .bind(session_id)
    .bind(user_id)
    .bind(SESSION_TTL_DAYS.to_string())
    .fetch_one(db)
    .await?;

    Ok(MintedSession {
        session_id,
        expires_at: row.0,
    })
}

/// Tuple-row of the `users` table columns needed by `touch_session`.
/// Split out as a type alias to tame clippy's `type_complexity` lint.
type UsersJoinRow = (i64, String, Option<String>, Option<String>, bool);

/// Refresh `last_seen_at` + `expires_at` on an active session and return the
/// joined `users` row. Returns `None` if the session is unknown, expired, or
/// revoked — the `Session` extractor then maps `None` to 401.
pub async fn touch_session(
    db: &PgPool,
    session_id: Uuid,
) -> Result<Option<SessionUser>, sqlx::Error> {
    // One round trip: UPDATE the session AND join users in a single CTE.
    // We do NOT use two queries because the first could succeed (session is
    // live) while a concurrent revocation slips in between.
    let row: Option<UsersJoinRow> = sqlx::query_as(
        "WITH touched AS ( \
             UPDATE sessions \
                SET last_seen_at = NOW(), \
                    expires_at   = NOW() + ($2 || ' days')::interval \
              WHERE session_id = $1 \
                AND revoked_at IS NULL \
                AND expires_at > NOW() \
              RETURNING user_id \
         ) \
         SELECT u.id, u.github_login, u.email, u.avatar_url, u.is_admin \
           FROM touched t \
           JOIN users u ON u.id = t.user_id",
    )
    .bind(session_id)
    .bind(SESSION_TTL_DAYS.to_string())
    .fetch_optional(db)
    .await?;

    Ok(row.map(|(id, login, email, avatar_url, is_admin)| SessionUser {
        id,
        login,
        email,
        avatar_url,
        is_admin,
    }))
}

/// Mark a session revoked. Idempotent: double-logout is a no-op. Returns
/// `true` if a row was actually revoked (for test assertions).
pub async fn revoke_session(db: &PgPool, session_id: Uuid) -> Result<bool, sqlx::Error> {
    let result =
        sqlx::query("UPDATE sessions SET revoked_at = NOW() WHERE session_id = $1 AND revoked_at IS NULL")
            .bind(session_id)
            .execute(db)
            .await?;
    Ok(result.rows_affected() > 0)
}

// ---------------------------------------------------------------------------
// OAuth state helpers (consumed by handlers/auth/github.rs)
// ---------------------------------------------------------------------------

/// Insert a fresh `oauth_state` row with the given opaque nonce and a 10-min
/// TTL. Called by `/auth/github/start` before redirecting the user to GitHub.
pub async fn insert_oauth_state(db: &PgPool, state: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO oauth_state (state, expires_at) \
         VALUES ($1, NOW() + interval '10 minutes')",
    )
    .bind(state)
    .execute(db)
    .await
    .map(|_| ())
}

/// Atomically mark an oauth_state nonce consumed. Returns `Ok(true)` if the
/// nonce was live + not-yet-consumed + unexpired (the only legal callback
/// path). Returns `Ok(false)` on miss (stale, replay, forgery) — callback
/// then emits `oauth_state_mismatch` 400.
pub async fn consume_oauth_state(db: &PgPool, state: &str) -> Result<bool, sqlx::Error> {
    let row: Option<(String,)> = sqlx::query_as(
        "UPDATE oauth_state \
            SET consumed_at = NOW() \
          WHERE state = $1 \
            AND consumed_at IS NULL \
            AND expires_at > NOW() \
          RETURNING state",
    )
    .bind(state)
    .fetch_optional(db)
    .await?;
    Ok(row.is_some())
}

// ---------------------------------------------------------------------------
// Axum extractor
// ---------------------------------------------------------------------------

impl<St> FromRequestParts<St> for Session
where
    St: AuthStateRef,
{
    type Rejection = AppError;

    fn from_request_parts(
        parts: &mut Parts,
        state: &St,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        let pool = state.pool().clone();
        let cookie_hdr = parts
            .headers
            .get(COOKIE)
            .and_then(|h| h.to_str().ok())
            .map(str::to_owned);

        async move {
            let session_id = cookie_hdr
                .as_deref()
                .and_then(parse_session_cookie)
                .ok_or(AppError::Unauthenticated)?;
            match touch_session(&pool, session_id).await? {
                Some(user) => Ok(Session(user)),
                None => Err(AppError::Unauthenticated),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Unit tests — cookie header + parse. Full DB round-trip lives in the
// integration test crate.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_cookie_header_contains_required_flags() {
        let id = Uuid::new_v4();
        let h = session_cookie_header(id);
        assert!(h.contains(&format!("{}={}", SESSION_COOKIE_NAME, id)));
        assert!(h.contains("HttpOnly"));
        assert!(h.contains("Secure"));
        assert!(h.contains("SameSite=Lax"));
        assert!(h.contains("Path=/"));
        assert!(h.contains(&format!("Max-Age={}", 7 * 24 * 3600)));
    }

    #[test]
    fn clear_session_cookie_header_is_max_age_zero() {
        let h = clear_session_cookie_header();
        assert!(h.starts_with(&format!("{}=;", SESSION_COOKIE_NAME)));
        assert!(h.contains("Max-Age=0"));
        assert!(h.contains("Path=/"));
        assert!(h.contains("SameSite=Lax"));
    }

    #[test]
    fn parse_session_cookie_round_trips() {
        let id = Uuid::new_v4();
        let hdr = format!("other=abc; {}={}; another=xyz", SESSION_COOKIE_NAME, id);
        assert_eq!(parse_session_cookie(&hdr), Some(id));
    }

    #[test]
    fn parse_session_cookie_returns_none_on_missing() {
        assert_eq!(parse_session_cookie("other=abc"), None);
        assert_eq!(parse_session_cookie(""), None);
    }

    #[test]
    fn parse_session_cookie_returns_none_on_bad_uuid() {
        let hdr = format!("{}=not-a-uuid", SESSION_COOKIE_NAME);
        assert_eq!(parse_session_cookie(&hdr), None);
    }

    #[test]
    fn session_ttl_is_seven_days() {
        assert_eq!(SESSION_TTL_DAYS, 7);
    }

    #[test]
    fn session_cookie_name_locked_to_siahub_session() {
        // D-54 — name is part of the cross-service contract; breaking it
        // silently logs every user out.
        assert_eq!(SESSION_COOKIE_NAME, "siahub_session");
    }
}
