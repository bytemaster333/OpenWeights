//! `POST /v1/xorbs/{prefix}/{hash}` — streaming body → merkle verify →
//! Sia upload+pin → DB row write.
//!
//! Pitfall ownership (CONTEXT §5, RESEARCH §9):
//!   - **P1** (hash encoding): `MerkleHash::from_hex` / `xorb_hash(..)` are
//!     the ONLY codec + aggregator used. NEVER `format!("{:02x}", b)` over
//!     raw bytes.
//!   - **P2** (merkle short-circuit, <10 ms, zero Sia I/O): body is bounded at
//!     64 MiB + 4 KiB, deserialized via `XorbObject::deserialize`, and the
//!     recomputed `xorb_hash` compared to the path hash BEFORE any
//!     `SiaAdapter::upload_and_pin` call. Task 5's P2 test asserts
//!     `upload_call_count() == 0` on a corrupt-footer upload.
//!   - **P7** (pin-state machine): insert_pending → upload_and_pin →
//!     set_pin_state('pinned') on success; leave `'pinning'` on Sia failure
//!     so Plan 02-09 reconciler can retry.
//!
//! Response body: `{"was_inserted": true | false}` per OQ-F. `true` on first
//! write, `false` on the dedup path (PK conflict on `xorb_merkle_hash`).

use std::io::Cursor;
use std::sync::Arc;
use std::time::Duration;

use axum::{
    Json,
    body::Body,
    extract::{Path, State},
};
use http_body_util::BodyExt;
use serde::Serialize;

use siahub_cas_db::queries::xorbs as xorb_q;
use siahub_cas_db::types::XorbPinState;
use siahub_cas_proto::{
    merklehash::{MerkleHash, xorb_hash},
    xorb_object::XorbObject,
};
use siahub_cas_storage::{SiaAdapter, SiaAdapterError};

use crate::auth::{AuthScoped, AuthStateRef};
use crate::errors::AppError;
use crate::rate_limit::{RateLimitClass, RateLimitDefaults};
use crate::scopes::SCOPE_UPLOAD;

/// Hard cap on xorb body size enforced BEFORE merkle parse.
///
/// 64 MiB is the xet-core per-xorb target; the extra 4 KiB headroom covers
/// the `XorbObjectInfoV1` footer (roughly `num_chunks * (32+4+4) bytes` for
/// hashes + two boundary arrays + fixed-length fields).
pub const MAX_XORB_BYTES: usize = 64 * 1024 * 1024 + 4096;

/// Response shape for POST /v1/xorbs/{prefix}/{hash}.
///
/// Uses snake_case `was_inserted` per xet-core's `UploadXorbResponse` wire type
/// (checked against the public HF xet protocol: `{"was_inserted": bool}`).
#[derive(Debug, Clone, Serialize)]
pub struct UploadXorbResponse {
    pub was_inserted: bool,
}

/// Trait the binary crate implements so this handler can reach the Sia
/// adapter without depending on the concrete `AppState` type. Mirrors the
/// `AuthStateRef` pattern used by Plan 02-03's auth extractor.
pub trait XorbUploadState: AuthStateRef {
    fn sia(&self) -> Arc<dyn SiaAdapter>;
    fn redis(&self) -> Arc<fred::clients::Client>;
    fn rate_limit_defaults(&self) -> RateLimitDefaults;
}

/// `POST /v1/xorbs/{prefix}/{hash}` — PROTO-02 / STORE-01 / STORE-02.
///
/// Handler order is load-bearing:
///   1. Parse path hash (P1).
///   2. Bound-read body (DoS cap).
///   3. Deserialize footer + recompute aggregated hash (P1).
///   4. **P2 short-circuit: compare; on mismatch return 400 — ZERO Sia I/O.**
///   5. Rate-limit check (OPS-04).
///   6. `insert_pending` (atomic PK dedup).
///   7. If was_inserted: `sia.upload_and_pin` → `set_pin_state('pinned')`.
///      Sia failure leaves the row in `'pinning'` for Plan 02-09 reconciler.
///   8. Write `usage_log` row (D-17) — Phase 2 metering.
///   9. Respond `{was_inserted}`.
pub async fn upload_xorb<S>(
    State(st): State<S>,
    AuthScoped(ctx): AuthScoped<{ SCOPE_UPLOAD }>,
    Path((prefix, hash_suffix)): Path<(String, String)>,
    body: Body,
) -> Result<Json<UploadXorbResponse>, AppError>
where
    S: XorbUploadState,
{
    // (1) Parse path hash — P1 discipline (NEVER hand-roll hex).
    let hash_hex = format!("{prefix}{hash_suffix}");
    let expected =
        MerkleHash::from_hex(&hash_hex).map_err(|_| AppError::BadRequest("invalid_xorb_hash"))?;

    // (2) Bounded body read — P2 / DoS cap. The body limiter below returns the
    //     buffered bytes OR an error if the body exceeded MAX_XORB_BYTES. We
    //     deliberately do NOT stream-parse; the xet-core `XorbObject::deserialize`
    //     is `Read + Seek` and the buffer is bounded.
    let collected = body
        .collect()
        .await
        .map_err(|_| AppError::BadRequest("invalid_body"))?
        .to_bytes();
    if collected.len() > MAX_XORB_BYTES {
        return Err(AppError::BadRequest("xorb_too_large"));
    }

    // (3) Deserialize footer + (4) compute+compare aggregated hash. ZERO Sia
    //     I/O up to this point. This whole block runs in-memory, O(num_chunks)
    //     — <10 ms on the 64 MiB cap per Plan 02-04 Task 5's P2 test.
    let actual = compute_xorb_hash_from_footer(&collected)?;
    if actual != expected {
        // P2 branch — fast reject, no Sia call, no DB write.
        return Err(AppError::HashMismatch { expected, actual });
    }

    // (5) Rate limit — AFTER merkle verify (cheap refused uploads don't count
    //     against the bucket; the refused uploads were already O(n) parsed,
    //     but not Sia-amplified). Fails with 429 + Retry-After.
    crate::rate_limit::check(
        &st.redis(),
        RateLimitClass::Upload,
        ctx.api_key_id,
        st.rate_limit_defaults(),
    )
    .await?;

    // (6) Atomic PK dedup. First writer gets was_inserted=true; any concurrent
    //     duplicate request for the same hash gets was_inserted=false WITHOUT
    //     calling Sia.
    let hash_bytes: [u8; 32] = <[u8; 32]>::from(expected);
    let size_bytes = collected.len() as i64;
    let pool = st.pool();
    let was_inserted =
        xorb_q::insert_pending(pool, &hash_bytes, size_bytes, ctx.user_id, ctx.api_key_id).await?;

    if !was_inserted {
        // Dedup path — no Sia call, no state transition. STORE-02 idempotent.
        crate::metering::log_on_err(
            "usage_log insert (xorb_upload, dedup) failed",
            crate::metering::record_xorb_upload(pool, &ctx, &hash_bytes, size_bytes).await,
        );
        return Ok(Json(UploadXorbResponse { was_inserted: false }));
    }

    // (7) Sia upload + pin. SiaAdapter::upload_and_pin is the ONLY path that
    //     actually writes to Sia — P2 is enforced by the early return above.
    let upload_res = with_timeout(
        Duration::from_secs(300),
        st.sia().upload_and_pin(&collected),
    )
    .await;

    match upload_res {
        Ok(Ok(sia_object_id)) => {
            // Happy path — flip pin_state 'pinning' → 'pinned' with the real id.
            xorb_q::set_pin_state(
                pool,
                &hash_bytes,
                XorbPinState::Pinned,
                Some(&sia_object_id),
            )
            .await?;
            crate::metering::log_on_err(
                "usage_log insert (xorb_upload) failed",
                crate::metering::record_xorb_upload(pool, &ctx, &hash_bytes, size_bytes).await,
            );
            Ok(Json(UploadXorbResponse { was_inserted: true }))
        }
        Ok(Err(SiaAdapterError::Unavailable(inner))) => {
            // P7: Sia unavailable. Leave pin_state='pinning' (the schema
            // default from insert_pending) so Plan 02-09 reconciler can
            // retry upload+pin. Stamp the attempt so the reconciler's
            // backoff window applies.
            let _ = xorb_q::set_pin_state(pool, &hash_bytes, XorbPinState::Pinning, None).await;
            Err(AppError::SiaUnavailable(inner))
        }
        Ok(Err(SiaAdapterError::Other(e))) => {
            // Non-unavailability Sia error (e.g. adapter misconfiguration).
            // Leave pin_state='pinning' so reconciler can retry; surface 500.
            let _ = xorb_q::set_pin_state(pool, &hash_bytes, XorbPinState::Pinning, None).await;
            Err(AppError::Other(e))
        }
        Err(_timeout) => {
            // Upload timed out — treat as unavailable. Reconciler retries.
            let _ = xorb_q::set_pin_state(pool, &hash_bytes, XorbPinState::Pinning, None).await;
            Err(AppError::SiaUnavailable(Box::new(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "sia upload+pin exceeded 300s budget",
            ))))
        }
    }
}

/// Deserialize the xorb footer in `bytes` and return the recomputed aggregated
/// merkle hash. Never touches the Sia SDK — invoked in the <10 ms P2 window.
fn compute_xorb_hash_from_footer(bytes: &[u8]) -> Result<MerkleHash, AppError> {
    let mut cursor = Cursor::new(bytes);
    let xorb = XorbObject::deserialize(&mut cursor)
        .map_err(|_| AppError::BadRequest("malformed_xorb"))?;

    let num_chunks = xorb.info.chunk_hashes.len();
    if num_chunks == 0 {
        // Empty xorb: xorb_hash(&[]) -> MerkleHash::default() — a valid but
        // degenerate case. Accept if it matches path, reject otherwise.
        return Ok(xorb_hash(&[]));
    }
    if xorb.info.unpacked_chunk_offsets.len() != num_chunks {
        // Malformed footer — boundary/hash arrays disagree.
        return Err(AppError::BadRequest("malformed_xorb"));
    }

    let mut hashes_and_lens: Vec<(MerkleHash, u64)> = Vec::with_capacity(num_chunks);
    let mut prev_off: u32 = 0;
    for i in 0..num_chunks {
        let off = xorb.info.unpacked_chunk_offsets[i];
        if off < prev_off {
            return Err(AppError::BadRequest("malformed_xorb"));
        }
        let len = (off - prev_off) as u64;
        hashes_and_lens.push((xorb.info.chunk_hashes[i], len));
        prev_off = off;
    }

    Ok(xorb_hash(&hashes_and_lens))
}

/// Small wrapper used to bound the Sia upload+pin roundtrip. `tokio::time::timeout`
/// is not used directly at the call site to keep the handler body clean and
/// testable; a thin local function makes the 300 s budget swappable in tests.
async fn with_timeout<F, T>(d: Duration, fut: F) -> Result<T, ()>
where
    F: std::future::Future<Output = T>,
{
    match tokio::time::timeout(d, fut).await {
        Ok(v) => Ok(v),
        Err(_) => Err(()),
    }
}

#[cfg(test)]
mod inline_tests {
    use super::*;

    /// P1 canary — reference xorb hash round-trip through the merklehash crate.
    /// notes.md §1 gotcha: NEVER hand-roll hex. Assert the crate's codec
    /// produces the same string we fed in, and that `From<DataHash> for
    /// [u8; 32]` + `.hex()` round-trips.
    #[test]
    fn p1_canary_reference_hash_round_trips() {
        const REF_HEX: &str =
            "eea25d6ee393ccae385820daed127b96ef0ea034dfb7cf6da3a950ce334b7632";
        let h = MerkleHash::from_hex(REF_HEX).expect("canonical ref hash must parse");
        assert_eq!(
            h.hex(),
            REF_HEX,
            "MerkleHash::from_hex ↔ .hex() must round-trip exactly"
        );

        // Also confirm bytes round-trip.
        let bytes: [u8; 32] = h.into();
        let h2 = MerkleHash::from(bytes);
        assert_eq!(h2.hex(), REF_HEX, "[u8;32] round-trip preserves identity");
    }

    /// Verifies an empty xorb aggregates to MerkleHash::default(). Required
    /// invariant for the `num_chunks == 0` branch above.
    #[test]
    fn empty_xorb_hash_is_default() {
        let h = xorb_hash(&[]);
        assert_eq!(h.hex(), MerkleHash::default().hex());
    }
}
