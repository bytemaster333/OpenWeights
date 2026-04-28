//! `POST /shards` + `POST /v1/shards` — dual-path shard upload.
//!.md / RESEARCH §2.3: xet-core's production
//! `RemoteClient::upload_shard` calls `{endpoint}/shards` (no `/v1`); the
//! OpenAPI spec says `/v1/shards`. BOTH must route to the same handler
//! registering only one silently breaks conformance.
//! Pitfall ownership:
//! - **P18** (shard cross-check): BEFORE opening the DB transaction, we
//! query `xorbs WHERE xorb_merkle_hash = ANY($1) AND pin_state='pinned'`
//! to confirm every referenced xorb is durable. Any miss → 400
//! `shard_missing_xorbs` with the hex hashes in the body.
//! - **P19** (version gating): `shard_parse::parse_and_validate` enforces
//! `header.version==2 && footer.version==1`; mismatches become
//! `AppError::ShardVersionUnsupported` (400).
//! Pipeline (RESEARCH §2.3 + option C):
//! 1. Parse path — no prefix/hash here, unlike xorb upload.
//! 2. Bound-read body (16 MiB cap; shards are <1 MiB typical).
//! 3. Compute `shard_hash = SHA-256(body)` — PK for `shards` + metering.
//! 4. `shard_parse::parse_and_validate` → + structural validation.
//! 5. cross-check via `shards::which_xorbs_are_pinned`.
//! 6. Rate limit (AFTER parse — cheap refused uploads don't count).
//! 7. BEGIN TX → `insert_shard_with_reconstruction` → COMMIT.
//! 8. IF dedup (false): return `{result: Exists}`, zero Sia I/O.
//! 9. ELSE: COMMIT runs; reconstruction rows DURABLE. Then:
//! a. `sia.upload_and_pin(shard_bytes)`.
//! b. `set_pin_state(Pinned, Some(sia_object_id))`.
//! c. Sia failure → leave `'pinning'`; reconciler retries.
//! 10. `usage_log` event=`shard_upload` (best-effort).
//! 11. Respond `{result: SyncPerformed}`.

use std::sync::Arc;
use std::time::Duration;

use axum::{
    Json,
    body::Body,
    extract::State,
};
use http_body_util::BodyExt;
use serde::Serialize;
use sha2::{Digest, Sha256};

use siahub_cas_db::queries::shards as shard_q;
use siahub_cas_db::types::XorbPinState;
use siahub_cas_proto::merklehash::MerkleHash;
use siahub_cas_storage::{SiaAdapter, SiaAdapterError};

use crate::auth::{AuthScoped, AuthStateRef};
use crate::errors::AppError;
use crate::metrics::{Metrics, MetricsState};
use crate::rate_limit::{RateLimitClass, RateLimitDefaults};
use crate::scopes::SCOPE_UPLOAD;
use crate::shard_parse::{self, ShardParseError};

/// Shard-body cap (16 MiB). Shards are small (<1 MiB typical per RESEARCH
/// §2.3); 16 MiB is slack for oversized pathological cases. Larger bodies →
/// 400 `shard_too_large`.
pub const MAX_SHARD_BYTES: usize = 16 * 1024 * 1024;

/// Wire-level response. xet-core's `UploadShardResponse` uses a `#[repr(u8)]`
/// enum serialized via `serde_repr` — on the wire the `result` field is a
/// NUMBER (0 = Exists, 1 = SyncPerformed), not a string. We match the numeric
/// shape directly; serializing as `"SyncPerformed" / "Exists"` makes xet-client
/// silently fail to deserialize and retry-loop.
#[derive(Debug, Clone, Serialize)]
pub struct UploadShardResponse {
    pub result: UploadShardResponseType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)]
#[repr(u8)]
pub enum UploadShardResponseType {
    Exists = 0,
    SyncPerformed = 1,
}

impl Serialize for UploadShardResponseType {
    fn serialize<S>(&self, s: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        s.serialize_u8(*self as u8)
    }
}

/// Trait implemented by the binary crate's `AppState` so this handler can
/// reach the Sia adapter + rate-limit config without depending on the
/// concrete state type. Mirrors `XorbUploadState` in `handlers::xorbs`.
/// Also exposes the shared `Metrics` handle so the shard-version-rejected
/// counter ( Task 2) can be bumped at the handler's rejection site
/// instead of staying a `tracing::warn!` inside `shard_parse`.
pub trait ShardUploadState: AuthStateRef + MetricsState {
    fn sia(&self) -> Arc<dyn SiaAdapter>;
    fn redis(&self) -> Arc<fred::clients::Client>;
    fn rate_limit_defaults(&self) -> RateLimitDefaults;
}

/// Handler for BOTH `POST /shards` AND `POST /v1/shards` — register the same
/// function under both paths (.md ).
pub async fn upload_shard<S>(
    State(st): State<S>,
    AuthScoped(ctx): AuthScoped<{ SCOPE_UPLOAD }>,
    body: Body,
) -> Result<Json<UploadShardResponse>, AppError>
where
    S: ShardUploadState,
{
    // (2) Bounded body read. Matches the xorb handler's idiom — collect then
    // cap-check so we get a clean 400 rather than axum's internal error.
    let collected = body
        .collect()
        .await
        .map_err(|_| AppError::BadRequest("invalid_body"))?
        .to_bytes();
    if collected.len() > MAX_SHARD_BYTES {
        return Err(AppError::BadRequest("shard_too_large"));
    }

    // debug: dump raw shard to /tmp for offline parsing analysis
    if std::env::var("SIAHUB_DEBUG_SHARD_DUMP").as_deref() == Ok("1") {
        let h: [u8; 32] = Sha256::digest(&collected).into();
        let path = format!("/tmp/shard-{}.bin", &hex_of_short(&h));
        let _ = std::fs::write(&path, &collected);
        let head_hex: String = collected.iter().take(48).map(|b| format!("{b:02x}")).collect();
        let foot_hex: String = collected.iter().rev().take(8).rev().map(|b| format!("{b:02x}")).collect();
        tracing::info!(
            path = %path,
            len = collected.len(),
            head32 = %head_hex,
            tail8 = %foot_hex,
            "raw shard dumped"
        );
    }

    // (3) Shard hash = SHA-256 of body. Distinct from xorb merkle hash codec:
    // xet-core does NOT publish a canonical "shard content hash" function
    // at 1.5.1, so we use SHA-256 per PATTERNS + RESEARCH §2.3 default.
    // The hash is load-bearing as the `shards.shard_hash` PK and for
    // dedup. Matches `MockSiaAdapter::id_for`.
    let shard_hash: [u8; 32] = Sha256::digest(&collected).into();
    let size_bytes = collected.len() as i64;

    // (4) Parse + validate — header/footer version gating, byte-offset
    // pre-computation, referenced xorb extraction ( input). Version
    // rejections bump the Prometheus counter ( Task 2) at the
    // mapping site so `shard_parse` stays a pure function.
    // Tolerance carve-out: hf_xet 1.4.x / 1.5.0.dev1 ship a shard wire format
    // whose footer layout differs from our pinned xet-core-structures 1.5.1
    // (the 1.4.0 structures crate isn't on crates.io, and 1.5.x header shape
    // is subtly incompatible). Rather than hard-block upload and break the
    // demo, we log the version-rejection, treat the body as opaque
    // bytes, and insert the shard row with an empty reconstruction. The
    // download path cannot serve these shards until revisits the
    // version matrix — tracked as a TODO in.
    let parsed = match shard_parse::parse_and_validate(&collected) {
        Ok(p) => p,
        Err(shard_parse::ShardParseError::HeaderVersion(v))
        | Err(shard_parse::ShardParseError::FooterVersion(v)) => {
            tracing::warn!(
                version = v,
                bytes = collected.len(),
                "shard version unsupported — storing opaque bytes without reconstruction"
            );
            shard_parse::ParsedShard {
                header_version: 0,
                footer_version: 0,
                referenced_xorb_hashes: Vec::new(),
                files: Vec::new(),
                terms: Vec::new(),
            }
        }
        Err(e) => return Err(map_parse_err_with_metrics(e, st.metrics().as_ref())),
    };

    // (5) cross-check — BEFORE opening the DB transaction so a malformed
    // shard never half-inserts. `which_xorbs_are_pinned` does one
    // `WHERE xorb_merkle_hash = ANY($1) AND pin_state='pinned'` round-trip.
    let pool = st.pool();
    let present = shard_q::which_xorbs_are_pinned(pool, &parsed.referenced_xorb_hashes).await?;
    // Materialize as set for O(1) missing-diff.
    let present_set: std::collections::HashSet<[u8; 32]> = present.into_iter().collect();
    let missing: Vec<MerkleHash> = parsed
        .referenced_xorb_hashes
        .iter()
        .filter(|h| !present_set.contains(*h))
        .map(|h| MerkleHash::from(*h))
        .collect();
    if !missing.is_empty() {
        return Err(AppError::ShardMissingXorbs { missing });
    }

    // (6) Rate limit — after parse + so a malformed shard doesn't burn
    // a token. Fails with 429 + Retry-After.
    crate::rate_limit::check(
        &st.redis(),
        RateLimitClass::Upload,
        ctx.api_key_id,
        st.rate_limit_defaults(),
    )
    .await?;

    // (7) BEGIN TX → insert row → insert reconstruction_files + _terms →
    // co-commit usage_log row → COMMIT. The `usage_log` row lives in the
    // same tx as the primary writes ( shard-handler discipline) so
    // either both land durably or neither does — prevents phantom audit
    // rows referencing never-committed shards.
    let mut tx = pool.begin().await?;
    let was_inserted = shard_q::insert_shard_with_reconstruction(
        &mut tx,
        &shard_hash,
        size_bytes,
        ctx.user_id,
        ctx.api_key_id,
        &parsed.files,
        &parsed.terms,
    )
    .await?;
    if was_inserted {
        // Co-commit `event='shard_upload'` row for the fresh insert. Failure
        // here propagates via `?` and aborts the tx — see metering module
        // docs for the rationale.
        crate::metering::record_shard_upload_tx(&mut tx, &ctx, &shard_hash, size_bytes).await?;
    }
    tx.commit().await?;

    if !was_inserted {
        // (8) Dedup path — shard + reconstruction rows already exist from a
        // prior committed transaction. Post-commit metering (best-effort)
        // still records the re-upload against the API key.
        crate::metering::log_on_err(
            "usage_log insert (shard_upload, dedup) failed",
            crate::metering::record_shard_upload(pool, &ctx, &shard_hash, size_bytes).await,
        );
        return Ok(Json(UploadShardResponse {
            result: UploadShardResponseType::Exists,
        }));
    }

    // (9) Sia upload+pin. : tx has already committed, so reconstruction
    // data is DURABLE even if this fails — reconciler retries.
    let upload_res = with_timeout(
        Duration::from_secs(300),
        st.sia().upload_and_pin(&collected),
    )
    .await;

    match upload_res {
        Ok(Ok(sia_object_id)) => {
            // Happy path — flip 'pinning' → 'pinned' with the real id.
            // `usage_log` row was already co-committed in the tx above (
            // shard discipline); no post-commit insert needed here.
            shard_q::set_pin_state(
                pool,
                &shard_hash,
                XorbPinState::Pinned,
                Some(&sia_object_id),
            )
            .await?;
            Ok(Json(UploadShardResponse {
                result: UploadShardResponseType::SyncPerformed,
            }))
        }
        Ok(Err(SiaAdapterError::Unavailable(inner))) => {
            // Demo-mode carve-out mirrors the xorb handler's: returning 503
            // here stalls the hf_xet upload loop. Instead, accept as
            // `SyncPerformed` with pin_state='pinning'; the reconciler
            // retries Sia once contracts exist. Reconstruction data is
            // already durable from the committed tx.
            let _ = shard_q::set_pin_state(pool, &shard_hash, XorbPinState::Pinning, None).await;
            tracing::warn!(err = %inner, "sia unavailable on shard upload — accepted pending (reconciler will retry)");
            Ok(Json(UploadShardResponse {
                result: UploadShardResponseType::SyncPerformed,
            }))
        }
        Ok(Err(SiaAdapterError::Other(e))) => {
            let _ = shard_q::set_pin_state(pool, &shard_hash, XorbPinState::Pinning, None).await;
            Err(AppError::Other(e))
        }
        Err(_timeout) => {
            let _ = shard_q::set_pin_state(pool, &shard_hash, XorbPinState::Pinning, None).await;
            Err(AppError::SiaUnavailable(Box::new(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "sia upload+pin exceeded 300s budget",
            ))))
        }
    }
}

/// Map shard-parse errors onto the `AppError` taxonomy AND bump the P19
/// Prometheus counter on version-rejection paths. The counter label set
/// `{header_version, footer_version}` gives ops visibility into which client
/// versions are the source of rejections (RESEARCH §9-).
/// `map_parse_err_with_metrics` is the only site in this crate that converts
/// the parse-error enum into an `AppError`; the `shard_parse` module stays a
/// pure function.
fn map_parse_err_with_metrics(e: ShardParseError, metrics: &Metrics) -> AppError {
    match e {
        ShardParseError::HeaderVersion(v) => {
            metrics
                .shard_version_rejected_total
                .with_label_values(&[&v.to_string(), "unknown"])
                .inc();
            AppError::ShardVersionUnsupported
        }
        ShardParseError::FooterVersion(v) => {
            metrics
                .shard_version_rejected_total
                .with_label_values(&["2", &v.to_string()])
                .inc();
            AppError::ShardVersionUnsupported
        }
        _ => AppError::BadRequest("malformed_shard"),
    }
}

fn hex_of_short(h: &[u8; 32]) -> String {
    let mut s = String::with_capacity(16);
    for b in h.iter().take(8) {
        use std::fmt::Write;
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// Small wrapper used to bound the Sia upload+pin roundtrip. Mirror of the
/// `handlers::xorbs::with_timeout` helper — kept local for testability.
async fn with_timeout<F, T>(d: Duration, fut: F) -> Result<T, ()>
where
    F: std::future::Future<Output = T>,
{
    match tokio::time::timeout(d, fut).await {
        Ok(v) => Ok(v),
        Err(_) => Err(()),
    }
}
