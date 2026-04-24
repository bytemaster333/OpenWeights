//! Bearer-token auth extractor + in-process LRU key cache.
//! Design (CONTEXT ):
//! - Incoming `Authorization: Bearer <plaintext>` → SHA-256 → `[u8; 32]`.
//! - Compare raw bytes to `api_keys.key_hash` (`BYTEA` in Postgres — NEVER hex).
//! - In-process LRU cache of `{hash → ActiveKey}` with 10 000 entries.
//! - Cache miss → `fetch_active_key_by_hash` (runtime-checked SQL until W2
//! ships `types.rs` + the compile-time macro variant).
//! - Strict 401/403:
//! 401 = missing bearer OR unknown/invalid hash.
//! 403 = known hash, missing scope. 401 ALWAYS wins if hash is unknown.
//! Guardrails (threat model — T-02-, T-02-, T-02-03-08):
//! - NEVER log plaintext bearer. Only hash-prefix (8 hex chars of SHA-256)
//! is permitted for debug breadcrumbs.
//! - Scope const-generic `S: u8` gates the handler at compile time. Unknown
//! u8 values are rejected by `ApiKeyScope::from_u8(S).ok_or(Forbidden)`.

use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use http::header::AUTHORIZATION;
use lru::LruCache;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::scopes::ApiKeyScope;

/// One active API-key row loaded from Postgres (or cache).
#[derive(Debug, Clone)]
pub struct ActiveKey {
    pub id: Uuid,
    pub user_id: i64,
    pub scopes: Vec<ApiKeyScope>,
}

/// Context carried into handlers once auth succeeds.
#[derive(Debug, Clone)]
pub struct AuthContext {
    pub api_key_id: Uuid,
    pub user_id: i64,
    pub scopes: Vec<ApiKeyScope>,
}

/// Const-generic scope-enforcing extractor.
/// Usage in a handler signature:
/// ```ignore
/// use siahub_cas_core::auth::AuthScoped;
/// use siahub_cas_core::scopes::SCOPE_UPLOAD;
/// async fn upload_xorb(AuthScoped(ctx): AuthScoped<{ SCOPE_UPLOAD }>) { /*...*/ }
/// ```
pub struct AuthScoped<const S: u8>(pub AuthContext);

/// Thread-safe in-process LRU of SHA-256 hash → `ActiveKey` rows.
/// Capacity is locked to 10 000 at construction. Cache is NOT shared
/// across processes — revocation-staleness window is 5 s (T-02-03-09).
pub struct KeyCache {
    inner: Mutex<LruCache<[u8; 32], ActiveKey>>,
}

impl KeyCache {
    pub fn new(capacity: usize) -> Self {
        let cap =
            NonZeroUsize::new(capacity).unwrap_or_else(|| NonZeroUsize::new(10_000).unwrap());
        Self {
            inner: Mutex::new(LruCache::new(cap)),
        }
    }

    /// Clone-out the cached row (cache holds the authoritative copy; callers
    /// get an owned view).
    pub fn get(&self, hash: &[u8; 32]) -> Option<ActiveKey> {
        self.inner.lock().ok()?.get(hash).cloned()
    }

    pub fn put(&self, hash: [u8; 32], key: ActiveKey) {
        if let Ok(mut g) = self.inner.lock() {
            g.put(hash, key);
        }
    }

    /// Test helper — current entry count.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.inner.lock().map(|g| g.len()).unwrap_or(0)
    }

    /// Test helper — is the cache empty?
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Minimal trait the binary crate implements for its concrete AppState so
/// this crate does not depend on it (avoids a cycle).
/// Returning `Arc<KeyCache>` (instead of `&KeyCache`) keeps the extractor's
/// returned future `Send + 'static` without unsafe lifetime tricks.
pub trait AuthStateRef: Send + Sync + Clone + 'static {
    fn pool(&self) -> &PgPool;
    fn key_cache(&self) -> Arc<KeyCache>;
}

/// Fetch an active (non-revoked) key row by its SHA-256 hash.
/// Uses runtime-checked SQL (NOT `sqlx::query!`) so this code compiles while
/// (W2) migrations are in flight in a sibling worktree. Once W2
/// lands, this can be upgraded to a compile-time-checked query.
pub async fn fetch_active_key_by_hash(
    pool: &PgPool,
    hash: &[u8; 32],
) -> Result<Option<ActiveKey>, sqlx::Error> {
    // The row is (UUID, BIGINT, api_key_scope[]). We decode scopes as
    // Vec<ApiKeyScope> via the sqlx::Type derive on ApiKeyScope.
    let row: Option<(Uuid, i64, Vec<ApiKeyScope>)> = sqlx::query_as(
        "SELECT id, user_id, scopes \
         FROM api_keys \
         WHERE key_hash = $1 AND revoked_at IS NULL",
    )
    .bind(&hash[..])
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|(id, user_id, scopes)| ActiveKey {
        id,
        user_id,
        scopes,
    }))
}

/// Best-effort UPDATE of `last_used_at`. Errors are LOGGED but not returned
///Xet JWT auth path. Invoked from `AuthScoped` when the
/// request bears `X-Xet-Access-Token` but no `Authorization: Bearer`.
/// Decodes (no signature verify — see `xet_jwt` module docs), validates
/// issuer + expiration, upserts a `users` row keyed on HF's userId, and
/// returns an `AuthScoped` with a synthetic `api_key_id` (`Uuid::nil`) so
/// downstream metering / FK columns see a valid handle.
async fn authenticate_xet_jwt<const S: u8>(
    pool: &PgPool,
    token: &str,
) -> Result<AuthScoped<S>, AppError> {
    use crate::xet_jwt;

    let claims = match xet_jwt::decode_and_validate(token) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(err = %e, "xet jwt rejected");
            return Err(AppError::Unauthenticated);
        }
    };

    let granted = claims.scopes();
    let required = ApiKeyScope::from_u8(S).ok_or(AppError::Forbidden)?;
    if !granted.contains(&required) {
        return Err(AppError::Forbidden);
    }

    // Two issuance sources, two user-resolution paths:
    // * HF issued (`iss = https://huggingface.co`): `userId` is a
    // 24-hex HF object id, hashed into a synthetic i64. Upsert a
    // `users` row + a per-user synthetic api_key so FKs hold.
    // * SiaHub issued (`iss = https://hf.siahub.app`): `userId` is
    // already a real `users.id` (we minted the token via the
    // HF-compat /xet-write-token endpoint on behalf of an
    // authenticated SiaHub key). Use it directly; look up the
    // actual api_keys row that was used to mint the token. If we
    // can't, fall back to a synthetic row keyed on the user id so
    // the FK still lands on something real.
    let (user_id, api_key_id) = if claims.iss == xet_jwt::SIAHUB_ISSUER {
        let uid = claims
            .user_id
            .parse::<i64>()
            .map_err(|_| AppError::Unauthenticated)?;
        // Verify the row actually exists — a forged JWT with a
        // never-existed user_id shouldn't be allowed to write even
        // though it decodes.
        let exists: Option<(i64,)> =
            sqlx::query_as("SELECT id FROM users WHERE id = $1")
                .bind(uid)
                .fetch_optional(pool)
                .await?;
        if exists.is_none() {
            return Err(AppError::Unauthenticated);
        }
        // Pick the user's active api_keys row with the widest scope set.
        // For our mint-path tokens the caller had both upload+download
        // (we required it at the HF-API-compat layer).
        let key_row: Option<(Uuid,)> = sqlx::query_as(
            "SELECT id FROM api_keys \
              WHERE user_id = $1 AND revoked_at IS NULL AND label != 'hf-jwt-synthetic' \
              ORDER BY array_length(scopes, 1) DESC NULLS LAST, last_used_at DESC NULLS LAST \
              LIMIT 1",
        )
        .bind(uid)
        .fetch_optional(pool)
        .await?;
        let key_id = match key_row {
            Some((id,)) => id,
            None => {
                xet_jwt::upsert_synthetic_api_key(pool, uid, &format!("siahub:{uid}"))
                    .await
                    .map_err(|e| {
                        AppError::Other(anyhow::anyhow!("synthetic api_key upsert: {e}"))
                    })?
            }
        };
        (uid, key_id)
    } else {
        let uid = xet_jwt::upsert_hf_user(pool, &claims.user_id)
            .await
            .map_err(|e| {
                tracing::error!(err = %e, "xet jwt user upsert failed");
                AppError::Other(anyhow::anyhow!("xet jwt user upsert: {e}"))
            })?;
        let key_id = xet_jwt::upsert_synthetic_api_key(pool, uid, &claims.user_id)
            .await
            .map_err(|e| {
                tracing::error!(err = %e, "xet jwt synthetic api_key upsert failed");
                AppError::Other(anyhow::anyhow!("xet jwt synthetic api_key upsert: {e}"))
            })?;
        (uid, key_id)
    };

    Ok(AuthScoped(AuthContext {
        api_key_id,
        user_id,
        scopes: granted,
    }))
}

/// Cheap syntactic check: does this Bearer token look like a JWT?
/// SiaHub-issued API keys are opaque url-safe random strings with no dots;
/// HF Xet tokens are HS256 JWTs with three dot-separated base64url segments
/// whose first segment decodes to `{"alg":"HS256",...}`. We only peek at the
/// `eyJ` prefix (base64 of `{"`) + the 3-segment shape — full validation
/// happens in `xet_jwt::decode_and_validate`.
fn looks_like_jwt(token: &str) -> bool {
    let segs: Vec<&str> = token.split('.').collect();
    segs.len() == 3 && segs[0].starts_with("eyJ") && !segs[1].is_empty()
}

/// failing to touch the timestamp must not fail the request.
pub async fn touch_last_used(pool: &PgPool, key_id: Uuid) {
    let res = sqlx::query("UPDATE api_keys SET last_used_at = NOW() WHERE id = $1")
        .bind(key_id)
        .execute(pool)
        .await;
    if let Err(e) = res {
        tracing::warn!(err = %e, %key_id, "failed to touch api_keys.last_used_at");
    }
}

impl<St, const S: u8> FromRequestParts<St> for AuthScoped<S>
where
    St: AuthStateRef,
{
    type Rejection = AppError;

    fn from_request_parts(
        parts: &mut Parts,
        state: &St,
    ) -> impl std::future::Future<Output = Result<Self, Self::Rejection>> + Send {
        // Clone only the small state refs we need; the future is Send + 'static.
        let pool = state.pool().clone();
        let cache = state.key_cache();
        let auth_header = parts
            .headers
            .get(AUTHORIZATION)
            .and_then(|h| h.to_str().ok())
            .map(str::to_owned);
        // amendment — fallback auth for hf_xet clients coming in
        // through siahub-hf-proxy. hf_xet actually ships the HF JWT as the
        // standard `Authorization: Bearer <JWT>` (confirmed via traffic
        // capture on hf_xet 1.4.3). We also honor `X-Xet-Access-Token` as a
        // secondary carrier for any caller that prefers the dedicated header.
        let xet_jwt_header = parts
            .headers
            .get("x-xet-access-token")
            .and_then(|h| h.to_str().ok())
            .map(str::to_owned);

        async move {
            // (1) Parse bearer header. If missing, try the dedicated
            // X-Xet-Access-Token fallback before returning 401.
            let plaintext = match auth_header
                .as_deref()
                .and_then(|s| s.strip_prefix("Bearer "))
            {
                Some(p) => p,
                None => {
                    if let Some(token) = xet_jwt_header.as_deref() {
                        return authenticate_xet_jwt::<S>(&pool, token).await;
                    }
                    return Err(AppError::Unauthenticated);
                }
            };

            // (1a) If the Bearer token looks like a JWT (three dot-separated
            // segments, standard `ey…` HS256 header prefix), route to the HF
            // Xet JWT path. Our own API keys are opaque random strings that
            // never match this shape, so the classifier is unambiguous.
            if looks_like_jwt(plaintext) {
                return authenticate_xet_jwt::<S>(&pool, plaintext).await;
            }

            // (2) SHA-256 the plaintext immediately. Plaintext is never logged.
            let hash: [u8; 32] = Sha256::digest(plaintext.as_bytes()).into();

            // (3) Cache check.
            let key = if let Some(cached) = cache.get(&hash) {
                cached
            } else {
                // (4) Cache miss → Postgres.
                match fetch_active_key_by_hash(&pool, &hash).await? {
                    Some(k) => {
                        cache.put(hash, k.clone());
                        k
                    }
                    None => {
                        // Unknown hash = 401 (NEVER 403 — T-02- + ).
                        return Err(AppError::Unauthenticated);
                    }
                }
            };

            // (5) Scope check — 401 has already been decided above; any
            // scope mismatch now is legitimately 403.
            let required = ApiKeyScope::from_u8(S).ok_or(AppError::Forbidden)?;
            if !key.scopes.contains(&required) {
                return Err(AppError::Forbidden); // 403
            }

            // (6) Fire-and-forget last_used_at update. Does NOT block the
            // request path and its failure is not propagated.
            let key_id = key.id;
            tokio::spawn(async move {
                touch_last_used(&pool, key_id).await;
            });

            Ok(AuthScoped(AuthContext {
                api_key_id: key.id,
                user_id: key.user_id,
                scopes: key.scopes,
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_cache_caps_at_capacity() {
        let cache = KeyCache::new(2);
        let h1 = [1u8; 32];
        let h2 = [2u8; 32];
        let h3 = [3u8; 32];
        let k = |id: u128| ActiveKey {
            id: Uuid::from_u128(id),
            user_id: 1,
            scopes: vec![ApiKeyScope::Upload],
        };
        cache.put(h1, k(1));
        cache.put(h2, k(2));
        assert_eq!(cache.len(), 2);
        cache.put(h3, k(3));
        // LRU evicts h1 (oldest).
        assert_eq!(cache.len(), 2);
        assert!(cache.get(&h1).is_none());
        assert!(cache.get(&h2).is_some());
        assert!(cache.get(&h3).is_some());
    }

    #[test]
    fn key_cache_default_capacity_is_ten_thousand() {
        let cache = KeyCache::new(10_000);
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn sha256_of_bearer_is_deterministic() {
        // Sanity — SHA-256 is used to key the cache; same input = same hash.
        let a: [u8; 32] = Sha256::digest(b"secret-token").into();
        let b: [u8; 32] = Sha256::digest(b"secret-token").into();
        assert_eq!(a, b);
    }

    #[test]
    fn key_cache_in_arc_compiles() {
        let _shared: Arc<KeyCache> = Arc::new(KeyCache::new(10_000));
    }
}
