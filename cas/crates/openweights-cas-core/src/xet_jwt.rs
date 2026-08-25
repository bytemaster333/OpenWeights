//! Hugging Face Xet JWT auth — amendment.
//! When a request carries no `Authorization: Bearer <plaintext>` header but
//! DOES carry an `X-Xet-Access-Token: <JWT>` header, this module is the
//! fallback identity path. It's how `hf upload` / `hf download` clients
//! authenticate to OpenWeights's CAS after the `openweights-hf-proxy` rewrites
//! `X-Xet-Cas-Url` to point here.
//! ## JWT trust model
//! HF issues a short-lived HS256 JWT per repo push; the payload looks like:
//! ```json
//! {
//! "repoId": "69e9049d91eb6fb3821af3b2",
//! "userId": "69e8fd473b7ac81c3649e201",
//! "access": "WRITE",
//! "iat": 1776882013,
//! "exp": 1776882913,
//! "iss": "https://huggingface.co"
//! }
//! ```
//! We do NOT verify the HS256 signature: HF uses a shared secret and does
//! not expose a public JWKS for Xet tokens. demo posture: we trust
//! the `iss` + `exp` + `iat` + payload shape as a strong-enough signal
//! when paired with network-level controls (proxy sits behind Caddy; only
//! the hf client + our own CAS speak this token flow; the proxy itself
//! only forwards what HF returned).
//! Production hardening paths (deferred to v1.1):
//! * Exchange the HF JWT for a OpenWeights-issued JWT at the proxy and sign
//!   it with a key our CAS holds.
//! * Subscribe to HF's public JWKS if/when they publish one.
//! * Downgrade forged tokens' blast radius by ratelimiting per-hf-user-id.

use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use sqlx::PgPool;

use crate::errors::AppError;
use crate::scopes::ApiKeyScope;

/// Canonical `iss` claims we accept. Anything else → reject. Two legal
/// issuers today:
/// * `https://huggingface.co` — tokens minted by HF for hf_xet clients
///   that came through openweights-hf-proxy.
/// * `https://hf.example.com` — tokens minted by THIS server when OpenWeights
///   plays the HF-API-compat role (migration 0008). Locked-down issuer
///   matching: we validate by exact string comparison, not substring.
const EXPECTED_ISSUERS: &[&str] = &["https://huggingface.co", "https://hf.example.com"];

/// The issuer openweights-cas writes into tokens it mints itself.
pub const OPENWEIGHTS_ISSUER: &str = "https://hf.example.com";

/// Maximum clock-skew tolerance between HF and our host, in seconds. 60 s
/// is generous; HF and our server should both track NTP.
const MAX_CLOCK_SKEW_SECS: u64 = 60;

/// The subset of the HF Xet JWT payload we care about. Extra fields (e.g.,
/// `usePrivateLink`, `hfCdnTier`) are ignored via `serde(default)` on the
/// struct boundary.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct XetJwtClaims {
    #[serde(rename = "repoId")]
    pub repo_id: String,
    #[serde(rename = "userId")]
    pub user_id: String,
    /// `"READ"` or `"WRITE"`.
    pub access: String,
    pub iat: i64,
    pub exp: i64,
    pub iss: String,
}

impl XetJwtClaims {
    /// Convert HF's `access` string to the scope set it implicitly grants.
    /// WRITE → Upload + Download; READ → Download only. Admin is NEVER
    /// reachable via this path (admin is GitHub-OAuth'd only).
    pub fn scopes(&self) -> Vec<ApiKeyScope> {
        match self.access.as_str() {
            "WRITE" => vec![ApiKeyScope::Upload, ApiKeyScope::Download],
            "READ" => vec![ApiKeyScope::Download],
            _ => vec![],
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum XetJwtError {
    #[error("jwt is not three dot-separated segments")]
    Malformed,
    #[error("jwt payload is not valid base64url")]
    PayloadNotBase64,
    #[error("jwt payload is not valid JSON")]
    PayloadNotJson,
    #[error("jwt issuer not in the accepted set")]
    WrongIssuer,
    #[error("jwt is expired")]
    Expired,
    #[error("jwt was issued in the future (clock skew > {MAX_CLOCK_SKEW_SECS}s)")]
    IssuedInFuture,
    #[error("jwt signature verification failed")]
    BadSignature,
}

impl From<XetJwtError> for AppError {
    fn from(_e: XetJwtError) -> Self {
        // Deliberately opaque to the client — we don't want to be a JWT
        // validator that emits detailed hints about what's wrong with an
        // attacker-supplied token. Log the reason at the call site.
        AppError::Unauthenticated
    }
}

/// Parse + validate a HF Xet JWT WITHOUT verifying the HS256 signature.
/// See the module-level doc comment for why this is acceptable for v1.
pub fn decode_and_validate(token: &str) -> Result<XetJwtClaims, XetJwtError> {
    // Three segments: header.payload.signature.
    let mut it = token.split('.');
    let _header = it.next().ok_or(XetJwtError::Malformed)?;
    let payload_b64 = it.next().ok_or(XetJwtError::Malformed)?;
    let _sig = it.next().ok_or(XetJwtError::Malformed)?;
    if it.next().is_some() {
        return Err(XetJwtError::Malformed);
    }

    let payload_bytes = B64URL
        .decode(payload_b64)
        .map_err(|_| XetJwtError::PayloadNotBase64)?;
    let claims: XetJwtClaims =
        serde_json::from_slice(&payload_bytes).map_err(|_| XetJwtError::PayloadNotJson)?;

    if !EXPECTED_ISSUERS.contains(&claims.iss.as_str()) {
        return Err(XetJwtError::WrongIssuer);
    }

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    if claims.exp < now {
        return Err(XetJwtError::Expired);
    }
    // `iat` in the future by more than MAX_CLOCK_SKEW_SECS → clock shenanigans.
    if claims.iat > now + MAX_CLOCK_SKEW_SECS as i64 {
        return Err(XetJwtError::IssuedInFuture);
    }

    Ok(claims)
}

/// Verify the HS256 signature of a token WE minted (`iss = OPENWEIGHTS_ISSUER`).
///
/// The generic HF Xet JWT path deliberately does not verify signatures (HF uses
/// a shared secret with no public JWKS — see the module docs). But tokens WE
/// issue via `mint_openweights_token` are HMAC-SHA256-signed with a key this
/// server holds (`XET_JWT_SIGNING_KEY`), so we MUST verify them: otherwise an
/// attacker can forge `{iss: OPENWEIGHTS_ISSUER, access: WRITE, userId: <any>}`
/// with a garbage signature and obtain write auth for any existing user.
///
/// Recomputes the HMAC over the `header.payload` signing input and compares it
/// to the token's signature segment in constant time. An empty `signing_key`
/// (feature not configured — in which case no OpenWeights token was ever minted)
/// yields `BadSignature`, so forged self-issued tokens are rejected either way.
pub fn verify_openweights_signature(token: &str, signing_key: &[u8]) -> Result<(), XetJwtError> {
    use hmac::{Hmac, Mac};

    if signing_key.is_empty() {
        return Err(XetJwtError::BadSignature);
    }

    let mut it = token.split('.');
    let header_b64 = it.next().ok_or(XetJwtError::Malformed)?;
    let payload_b64 = it.next().ok_or(XetJwtError::Malformed)?;
    let sig_b64 = it.next().ok_or(XetJwtError::Malformed)?;
    if it.next().is_some() {
        return Err(XetJwtError::Malformed);
    }

    let expected_sig = B64URL
        .decode(sig_b64)
        .map_err(|_| XetJwtError::BadSignature)?;
    let signing_input = format!("{header_b64}.{payload_b64}");

    type HmacSha256 = Hmac<Sha256>;
    let mut mac =
        HmacSha256::new_from_slice(signing_key).map_err(|_| XetJwtError::BadSignature)?;
    mac.update(signing_input.as_bytes());
    // `verify_slice` is a constant-time comparison.
    mac.verify_slice(&expected_sig)
        .map_err(|_| XetJwtError::BadSignature)
}

/// Deterministically map HF's 24-hex userId string to a positive i64.
/// The `users.id` column is BIGINT PRIMARY KEY; we need a numeric to fit
/// alongside GitHub's numeric user IDs without colliding.
/// Collision math: birthday bound at ~2^31 distinct HF users before a
/// 50% collision chance — we won't hit that at v1 demo scale.
pub fn hf_user_id_to_i64(hf_user_id: &str) -> i64 {
    let digest: [u8; 32] = Sha256::digest(hf_user_id.as_bytes()).into();
    let first8: [u8; 8] = digest[..8].try_into().expect("digest is 32 bytes");
    // Mask the sign bit so the result is always positive; Postgres BIGINT
    // is signed so negative values would round-trip OK but some downstream
    // queries expect non-negative IDs.
    (u64::from_be_bytes(first8) & 0x7FFF_FFFF_FFFF_FFFF) as i64
}

/// Upsert a user row for the HF-identified user, returning the `id` that
/// downstream rows (xorbs, shards, usage_log) should FK to.
/// The `github_login` column is NOT NULL UNIQUE; we stuff `hf:<userId>`
/// there. No real github login can start with `hf:`.
pub async fn upsert_hf_user(pool: &PgPool, hf_user_id: &str) -> Result<i64, sqlx::Error> {
    let numeric_id = hf_user_id_to_i64(hf_user_id);
    let synthetic_login = format!("hf:{}", hf_user_id);

    // Using hf_user_id as the CONFLICT target lets us re-key a row if the
    // hash-derived `id` ever collides with a GitHub user (which would have
    // hf_user_id NULL). In that case we'd pick a different numeric id
    // TODO v1.1. For now, collision is a 2^-32 event per distinct HF user.
    // `ON CONFLICT (hf_user_id) WHERE hf_user_id IS NOT NULL` matches the
    // partial unique index in migration 0007 (unique only for non-NULL values,
    // so GitHub users don't collide on NULL). Omitting the WHERE predicate
    // makes Postgres reject the statement ("no unique constraint matching").
    let row: (i64,) = sqlx::query_as(
        r#"
        INSERT INTO users (id, github_login, hf_user_id)
        VALUES ($1, $2, $3)
        ON CONFLICT (hf_user_id) WHERE hf_user_id IS NOT NULL
        DO UPDATE SET
            github_login = EXCLUDED.github_login
        RETURNING id
        "#,
    )
    .bind(numeric_id)
    .bind(&synthetic_login)
    .bind(hf_user_id)
    .fetch_one(pool)
    .await?;

    Ok(row.0)
}

/// Ensure a synthetic `api_keys` row exists for an HF-JWT-authenticated user
/// and return its UUID.
/// Downstream tables (`xorbs`, `shards`, `usage_log`) all FK to
/// `api_keys.id`. Because HF-JWT auth doesn't correspond to a user-minted
/// OpenWeights API key, we conjure one sentinel row per user so the FK holds.
/// The row:
/// * `key_hash` — SHA-256 of `"hf-jwt:<hf_user_id>"`. Deterministic, so
///   concurrent requests for the same HF user converge on the same row
///   via the existing `api_keys_key_hash_key` UNIQUE constraint.
/// * `scopes` — `{upload, download}` (HF Xet WRITE tokens imply both).
/// * `label` — `"hf-jwt-synthetic"` so operators can identify these
///   rows when auditing `/admin/keys`.
///   Returns the synthetic key id to stamp into `AuthContext.api_key_id`.
pub async fn upsert_synthetic_api_key(
    pool: &PgPool,
    user_id: i64,
    hf_user_id: &str,
) -> Result<uuid::Uuid, sqlx::Error> {
    use crate::scopes::ApiKeyScope;

    let key_hash: [u8; 32] =
        Sha256::digest(format!("hf-jwt:{hf_user_id}").as_bytes()).into();
    let scopes: Vec<ApiKeyScope> = vec![ApiKeyScope::Upload, ApiKeyScope::Download];

    let row: (uuid::Uuid,) = sqlx::query_as(
        r#"
        INSERT INTO api_keys (user_id, key_hash, scopes, label)
        VALUES ($1, $2, $3, 'hf-jwt-synthetic')
        ON CONFLICT (key_hash)
        DO UPDATE SET last_used_at = NOW()
        RETURNING id
        "#,
    )
    .bind(user_id)
    .bind(&key_hash[..])
    .bind(&scopes)
    .fetch_one(pool)
    .await?;

    Ok(row.0)
}

/// Mint a OpenWeights-issued Xet JWT for an HF-compat upload/download session.
/// Used by the HF-API-compat `xet-write-token` / `xet-read-token` handlers
/// (migration 0008). hf_xet treats these as opaque bearer tokens — what
/// matters is that they survive our own `decode_and_validate` (for write
/// uploads against `/v1/xorbs/*` + `/v1/shards` on the same host) AND parse
/// in hf_xet's own validator.
/// We only populate the fields hf_xet reads: `repoId` (used in log lines),
/// `userId` (hf_xet may log), `access` (WRITE/READ), `iat`, `exp`, `iss`.
/// The HS256 signature uses `signing_key`; we do NOT verify signatures on
/// the read side (neither HF nor us, per module docs), so key rotation is
/// transparent to already-issued tokens.
/// Returned string is the compact-form `header.payload.signature`.
pub fn mint_openweights_token(
    signing_key: &[u8],
    repo_id: &str,
    user_id_str: &str,
    access: OpenWeightsAccess,
    ttl_secs: u64,
) -> Result<String, anyhow::Error> {
    use hmac::{Hmac, Mac};
    use serde_json::json;

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let exp = now + ttl_secs as i64;

    let access_str = match access {
        OpenWeightsAccess::Write => "WRITE",
        OpenWeightsAccess::Read => "READ",
    };

    // Match HF's header shape byte-for-byte: {"alg":"HS256"} with no `typ`.
    // The parser ignores the header contents but downstream tooling that
    // inspects tokens expects this compact form.
    let header_json = br#"{"alg":"HS256"}"#;
    let payload = json!({
        "repoId": repo_id,
        "userId": user_id_str,
        "access": access_str,
        "iat": now,
        "exp": exp,
        "iss": OPENWEIGHTS_ISSUER,
    });
    let payload_json = serde_json::to_vec(&payload)?;

    let header_b64 = B64URL.encode(header_json);
    let payload_b64 = B64URL.encode(payload_json);
    let signing_input = format!("{header_b64}.{payload_b64}");

    type HmacSha256 = Hmac<Sha256>;
    let mut mac = HmacSha256::new_from_slice(signing_key)
        .map_err(|e| anyhow::anyhow!("hmac key: {e}"))?;
    mac.update(signing_input.as_bytes());
    let sig = mac.finalize().into_bytes();
    let sig_b64 = B64URL.encode(sig);

    Ok(format!("{signing_input}.{sig_b64}"))
}

/// Scope marker for `mint_openweights_token`. Maps 1:1 to HF's `access` claim
/// string; `Write` grants upload+download, `Read` grants download only.
#[derive(Debug, Clone, Copy)]
pub enum OpenWeightsAccess {
    Write,
    Read,
}

#[cfg(test)]
mod tests {
    use super::*;

    // A real token captured from HF's API during a `hf upload` flow. This
    // is long-expired (iat 2026-04-22) so it's safe to commit. Kept to
    // pin the parser shape against the real-world response.
    const SAMPLE_TOKEN: &str = "eyJhbGciOiJIUzI1NiJ9.eyJyZXBvSWQiOiI2OWU5MDQ5ZDkxZWI2ZmIzODIxYWYzYjIiLCJ1c2VySWQiOiI2OWU4ZmQ0NzNiN2FjODFjMzY0OWUyMDEiLCJhY2Nlc3MiOiJXUklURSIsInVzZVByaXZhdGVMaW5rIjpmYWxzZSwiaGZDZG5UaWVyIjoiYXV0aGVudGljYXRlZCIsInJlcG9fc3RvcmFnZV90eXBlIjoiZ2l0IiwiaWF0IjoxNzc2ODgyMDEzLCJleHAiOjE3NzY4ODI5MTMsImlzcyI6Imh0dHBzOi8vaHVnZ2luZ2ZhY2UuY28ifQ.DUOACjJKAkp9EFz9aViw5_5vRaYcoyGfKkSFzYw5SCQ";

    #[test]
    fn decode_extracts_user_and_scope() {
        // The expired sample will fail the `exp` check — extract the claims
        // via manual decode path to verify payload shape independently.
        let mut it = SAMPLE_TOKEN.split('.');
        let _h = it.next().unwrap();
        let payload_b64 = it.next().unwrap();
        let bytes = B64URL.decode(payload_b64).unwrap();
        let claims: XetJwtClaims = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(claims.user_id, "69e8fd473b7ac81c3649e201");
        assert_eq!(claims.access, "WRITE");
        assert_eq!(claims.iss, "https://huggingface.co");
        assert_eq!(
            claims.scopes(),
            vec![ApiKeyScope::Upload, ApiKeyScope::Download]
        );
    }

    #[test]
    fn expired_token_is_rejected() {
        // Craft a JWT with iat+exp in 1970 so it's deterministically past.
        // Header `{"alg":"HS256"}` + payload with expired exp + fake sig.
        let header = B64URL.encode(br#"{"alg":"HS256"}"#);
        let payload = B64URL.encode(
            br#"{"repoId":"r","userId":"u","access":"WRITE","iat":1000,"exp":2000,"iss":"https://huggingface.co"}"#,
        );
        let token = format!("{header}.{payload}.deadbeef");
        let err = decode_and_validate(&token).unwrap_err();
        assert!(matches!(err, XetJwtError::Expired), "got {err:?}");
    }

    #[test]
    fn wrong_issuer_is_rejected() {
        let header = B64URL.encode(br#"{"alg":"HS256"}"#);
        let far_future_exp = i64::MAX / 2;
        let payload = B64URL.encode(
            format!(
                r#"{{"repoId":"r","userId":"u","access":"WRITE","iat":1000,"exp":{far_future_exp},"iss":"https://evil.example.com"}}"#
            )
            .as_bytes(),
        );
        let token = format!("{header}.{payload}.deadbeef");
        let err = decode_and_validate(&token).unwrap_err();
        assert!(matches!(err, XetJwtError::WrongIssuer), "got {err:?}");
    }

    #[test]
    fn malformed_shape_is_rejected() {
        assert!(matches!(
            decode_and_validate("not-a-jwt").unwrap_err(),
            XetJwtError::Malformed
        ));
        assert!(matches!(
            decode_and_validate("one.two").unwrap_err(),
            XetJwtError::Malformed
        ));
    }

    #[test]
    fn openweights_minted_token_round_trips_through_decode_and_validate() {
        // Mint a short-TTL WRITE token with a random-ish key and feed it
        // back through the validator. The key is NOT used for validation
        // (per module docs — no signature verify), but we still exercise
        // the HMAC path so a signing regression surfaces here.
        let key = b"xet-jwt-test-key-not-real-entropy";
        let token = mint_openweights_token(
            key,
            "user/demo",
            "2279157115906683656",
            OpenWeightsAccess::Write,
            60,
        )
        .expect("mint");
        let claims = decode_and_validate(&token).expect("validate");
        assert_eq!(claims.repo_id, "user/demo");
        assert_eq!(claims.user_id, "2279157115906683656");
        assert_eq!(claims.access, "WRITE");
        assert_eq!(claims.iss, OPENWEIGHTS_ISSUER);
        assert_eq!(
            claims.scopes(),
            vec![ApiKeyScope::Upload, ApiKeyScope::Download]
        );
    }

    #[test]
    fn openweights_signature_verifies_and_rejects_forgeries() {
        let key = b"xet-jwt-test-key-not-real-entropy";
        let token = mint_openweights_token(
            key,
            "user/demo",
            "2279157115906683656",
            OpenWeightsAccess::Write,
            60,
        )
        .expect("mint");

        // Genuine token, correct key → verifies.
        verify_openweights_signature(&token, key).expect("genuine token must verify");

        // Wrong key → rejected.
        assert!(matches!(
            verify_openweights_signature(&token, b"a-different-key"),
            Err(XetJwtError::BadSignature)
        ));

        // Empty key (feature unconfigured) → rejected, never silently trusted.
        assert!(matches!(
            verify_openweights_signature(&token, b""),
            Err(XetJwtError::BadSignature)
        ));

        // FORGERY: attacker forges a WRITE token for iss=OPENWEIGHTS_ISSUER with
        // a garbage signature. decode_and_validate would accept the claims, but
        // signature verification must reject it (this is the write-auth bypass).
        let header = B64URL.encode(br#"{"alg":"HS256"}"#);
        let far_future = i64::MAX / 2;
        let payload = B64URL.encode(
            format!(
                r#"{{"repoId":"victim/repo","userId":"1","access":"WRITE","iat":1000,"exp":{far_future},"iss":"{OPENWEIGHTS_ISSUER}"}}"#
            )
            .as_bytes(),
        );
        let forged = format!("{header}.{payload}.Zm9yZ2VkLXNpZw");
        // The claims themselves pass decode_and_validate (issuer/exp are legal)...
        decode_and_validate(&forged).expect("forged claims decode (issuer+exp legal)");
        // ...but the signature does NOT verify, which is what closes the hole.
        assert!(matches!(
            verify_openweights_signature(&forged, key),
            Err(XetJwtError::BadSignature)
        ));
    }

    #[test]
    fn hf_user_id_mapping_is_deterministic_and_positive() {
        let a = hf_user_id_to_i64("69e8fd473b7ac81c3649e201");
        let b = hf_user_id_to_i64("69e8fd473b7ac81c3649e201");
        assert_eq!(a, b, "same input → same id");
        assert!(a > 0, "result must be positive; got {a}");
        // Distinct input → different id (extremely likely).
        let c = hf_user_id_to_i64("different-hf-user");
        assert_ne!(a, c);
    }
}
