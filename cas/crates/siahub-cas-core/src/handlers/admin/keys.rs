//! `POST/GET/DELETE /admin/keys[/{id}]` — API key CRUD for the session user.
//! Backs..04. Response contract locked in 04-KEY-DECISIONS §2:
//! * POST → `201 {id, name, scope, masked_prefix, plaintext_key, created_at}`
//!plaintext returned EXACTLY ONCE per.
//! * GET → `200 {keys: [{id, name, scope, masked_prefix, created_at,
//! last_used_at},...]}` — plaintext NEVER included.
//! * DELETE → `204 No Content` — sets `revoked_at = NOW`. 's auth
//! extractor (`siahub_cas_core::auth`) respects `revoked_at IS NULL` so
//! propagation is immediate for uncached bearer paths. The in-process LRU
//! cache in the bearer path has a 5-s TTL bound ( SLO) documented
//! in `auth.rs` T-02-.
//! Scope mapping — the console contract uses `"read"|"write"|"admin"` while
//! the DB enum is `upload|download|admin` (migration 0001). We translate at
//! the handler boundary:
//! * `"read"` ↔ `ApiKeyScope::Download`
//! * `"write"` ↔ `ApiKeyScope::Upload`
//! * `"admin"` ↔ `ApiKeyScope::Admin`
//! Translation is deliberately local to this module so the DB enum can
//! expand without churning the browser-facing wire format.
//! Security:
//! * Plaintext key = 32 URL-safe random bytes (base64url, no padding).
//! * `key_hash` stored is raw SHA-256([u8; 32]) — NOT hex ( sibling
//! convention; matches the bearer path in `auth.rs`).
//! * `masked_prefix = "<first 8 chars of plaintext>..."` captured at
//! creation time; stored so list endpoint can render a non-secret label.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Json, response::IntoResponse};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use siahub_cas_db::types::ApiKeyScope as DbApiKeyScope;

use crate::auth::AuthStateRef;
use crate::errors::AppError;
use crate::session::Session;

// ---------------------------------------------------------------------------
// Wire types
// ---------------------------------------------------------------------------

/// Scope enum as the console sees it. Do NOT re-export `ApiKeyScope` from
/// `scopes.rs` — that leaks the DB-native labels (`upload|download`).
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ConsoleScope {
    Read,
    Write,
    Admin,
}

impl ConsoleScope {
    fn to_db(self) -> DbApiKeyScope {
        match self {
            Self::Read => DbApiKeyScope::Download,
            Self::Write => DbApiKeyScope::Upload,
            Self::Admin => DbApiKeyScope::Admin,
        }
    }

    fn from_db(s: DbApiKeyScope) -> Self {
        match s {
            DbApiKeyScope::Download => Self::Read,
            DbApiKeyScope::Upload => Self::Write,
            DbApiKeyScope::Admin => Self::Admin,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct CreateKeyRequest {
    pub name: String,
    pub scope: ConsoleScope,
}

#[derive(Debug, Serialize)]
pub struct CreateKeyResponse {
    pub id: Uuid,
    pub name: String,
    pub scope: ConsoleScope,
    pub masked_prefix: String,
    /// **: returned EXACTLY ONCE in this response. Never logged; never
    /// stored in any field other than the response body; never included in
    /// any other endpoint's output.** Integration test `admin_endpoints.rs`
    /// greps the list-keys response body for this literal to enforce.
    pub plaintext_key: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct KeyListItem {
    pub id: Uuid,
    pub name: Option<String>,
    pub scope: ConsoleScope,
    pub masked_prefix: Option<String>,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize)]
pub struct ListKeysResponse {
    pub keys: Vec<KeyListItem>,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `POST /admin/keys` — create a fresh API key for the session user.
pub async fn create_key<S: AuthStateRef>(
    Session(user): Session,
    State(st): State<S>,
    Json(req): Json<CreateKeyRequest>,
) -> Result<(StatusCode, Json<CreateKeyResponse>), AppError> {
    // Validate name — keep v1 simple: trim + reject empty + cap at 80 chars.
    let name = req.name.trim().to_string();
    if name.is_empty() {
        return Err(AppError::BadRequest("name must not be empty"));
    }
    if name.len() > 80 {
        return Err(AppError::BadRequest("name must be <= 80 chars"));
    }

    // Generate 32B URL-safe random — CSPRNG via getrandom.
    let mut raw = [0u8; 32];
    getrandom::getrandom(&mut raw).map_err(|e| AppError::Other(anyhow::anyhow!(e)))?;
    let plaintext = URL_SAFE_NO_PAD.encode(raw);

    // SHA-256([u8; 32]) — stored raw BYTEA per 0001_initial.sql.
    let key_hash: [u8; 32] = Sha256::digest(plaintext.as_bytes()).into();

    // Masked prefix — first 8 chars + "..." ellipsis. Non-secret by design
    // (8 chars of 43-char base64url leave ~2^192 guessing entropy).
    let masked_prefix = format!("{}...", &plaintext[..8]);

    let db_scope = req.scope.to_db();

    // INSERT key — PK gen_random_uuid in migration 0001.
    let row: (Uuid, DateTime<Utc>) = sqlx::query_as(
        "INSERT INTO api_keys (user_id, key_hash, scopes, label, masked_prefix) \
         VALUES ($1, $2, ARRAY[$3::api_key_scope], $4, $5) \
         RETURNING id, created_at",
    )
    .bind(user.id)
    .bind(&key_hash[..])
    .bind(db_scope)
    .bind(&name)
    .bind(&masked_prefix)
    .fetch_one(st.pool())
    .await?;

    Ok((
        StatusCode::CREATED,
        Json(CreateKeyResponse {
            id: row.0,
            name,
            scope: req.scope,
            masked_prefix,
            plaintext_key: plaintext,
            created_at: row.1,
        }),
    ))
}

/// `GET /admin/keys` — list non-revoked keys for the session user.
/// **MUST NOT include `plaintext_key`** — the integration test greps response
/// bodies for that substring to enforce.
pub async fn list_keys<S: AuthStateRef>(
    Session(user): Session,
    State(st): State<S>,
) -> Result<Json<ListKeysResponse>, AppError> {
    // Type alias keeps the `query_as` annotation below readable.
    type KeyRow = (
        Uuid,
        Option<String>,
        Vec<DbApiKeyScope>,
        Option<String>,
        DateTime<Utc>,
        Option<DateTime<Utc>>,
    );
    let rows: Vec<KeyRow> = sqlx::query_as(
        "SELECT id, label, scopes, masked_prefix, created_at, last_used_at \
           FROM api_keys \
          WHERE user_id = $1 AND revoked_at IS NULL \
          ORDER BY created_at DESC",
    )
    .bind(user.id)
    .fetch_all(st.pool())
    .await?;

    let keys = rows
        .into_iter()
        .map(
            |(id, label, scopes, masked_prefix, created_at, last_used_at)| {
                // Take the first scope — v1 always creates single-scope keys;
                // a legacy row with multiple scopes surfaces the first one
                // (console won't see these in practice).
                let scope = scopes
                    .first()
                    .copied()
                    .map(ConsoleScope::from_db)
                    .unwrap_or(ConsoleScope::Read);
                KeyListItem {
                    id,
                    name: label,
                    scope,
                    masked_prefix,
                    created_at,
                    last_used_at,
                }
            },
        )
        .collect();

    Ok(Json(ListKeysResponse { keys }))
}

/// `DELETE /admin/keys/{id}` — revoke a key owned by the session user.
/// : propagation < 5s SLO. The in-process LRU in `auth.rs` does not
/// observe revocation directly; its TTL is bounded to 5 s via the
/// auth-extractor retry loop that re-validates cache hits against Postgres
/// when the session cache age exceeds the SLO window. Tests assert that a
/// subsequent bearer call with the revoked key returns 401 within the same
/// process lifetime.
pub async fn revoke_key<S: AuthStateRef>(
    Session(user): Session,
    State(st): State<S>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let res = sqlx::query(
        "UPDATE api_keys \
            SET revoked_at = NOW() \
          WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL",
    )
    .bind(id)
    .bind(user.id)
    .execute(st.pool())
    .await?;

    if res.rows_affected() == 0 {
        // Either not owned by this user or already revoked. 404 is the
        // honest answer — the key is not findable *as an active one you
        // own* — and avoids leaking existence of keys belonging to other
        // users.
        return Err(AppError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_maps_read_to_download() {
        assert_eq!(ConsoleScope::Read.to_db(), DbApiKeyScope::Download);
        assert_eq!(ConsoleScope::Write.to_db(), DbApiKeyScope::Upload);
        assert_eq!(ConsoleScope::Admin.to_db(), DbApiKeyScope::Admin);
    }

    #[test]
    fn scope_round_trips_db_to_console() {
        assert_eq!(
            ConsoleScope::from_db(DbApiKeyScope::Download),
            ConsoleScope::Read
        );
        assert_eq!(
            ConsoleScope::from_db(DbApiKeyScope::Upload),
            ConsoleScope::Write
        );
        assert_eq!(
            ConsoleScope::from_db(DbApiKeyScope::Admin),
            ConsoleScope::Admin
        );
    }

    #[test]
    fn scope_serde_lowercase() {
        let s = serde_json::to_string(&ConsoleScope::Read).unwrap();
        assert_eq!(s, "\"read\"");
        let r: ConsoleScope = serde_json::from_str("\"write\"").unwrap();
        assert_eq!(r, ConsoleScope::Write);
    }
}
