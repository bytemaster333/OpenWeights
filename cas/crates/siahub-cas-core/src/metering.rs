//! Metering facade — thin wrapper around `siahub_cas_db::queries::usage_log`.
//! / /. Every xorb_upload, shard_upload, and
//! reconstruction event writes exactly one row to `usage_log` synchronously
//! from the handler's request path. This module is the ONE convergence point
//! for every metering call-site — OQ-K (bytes/min pivot if the
//! req/min bucket misbehaves) rewrites these three functions only, without
//! touching handlers.
//! Error semantics ( + plan Task 1):
//! * `record_xorb_upload`, `record_shard_upload`, `record_reconstruction`
//! all return `Result<, sqlx::Error>`, but the CALLER decides propagation.
//! All handlers treat post-commit metering as best-effort: they
//! log the error via `tracing::warn!` and return 200 anyway (the user's
//! upload already succeeded; losing an audit row is acceptable v1
//! behavior).
//! * The shard handler's `record_shard_upload_tx` co-commits inside the
//! reconstruction tx; propagating the error aborts the tx, which is
//! intentional (if metering fails mid-commit, the whole state is corrupt).
//! The `AuthContext` argument keeps every call-site consistent with the
//! bearer-auth extractor (`AuthScoped<const S: u8>`), so no handler has to
//! reach into `api_key_id` + `user_id` individually.

use sqlx::{PgPool, Postgres, Transaction};

use siahub_cas_db::queries::usage_log;

use crate::auth::AuthContext;

/// Record a successful xorb upload. Called by on both the
/// insert path and the dedup path (was_inserted=false) so idempotent
/// re-uploads still accrue against the caller's API key.
pub async fn record_xorb_upload(
    pool: &PgPool,
    ctx: &AuthContext,
    xorb_hash: &[u8; 32],
    bytes: i64,
) -> Result<(), sqlx::Error> {
    usage_log::insert_xorb_upload(pool, ctx.api_key_id, ctx.user_id, xorb_hash, bytes).await
}

/// Record a successful shard upload (post-commit variant — dedup
/// branch + Sia-success branch both use this). For the tx-inline variant used
/// during the shard-insert transaction, see [`record_shard_upload_tx`].
pub async fn record_shard_upload(
    pool: &PgPool,
    ctx: &AuthContext,
    shard_hash: &[u8; 32],
    bytes: i64,
) -> Result<(), sqlx::Error> {
    usage_log::insert_shard_upload(pool, ctx.api_key_id, ctx.user_id, shard_hash, bytes).await
}

/// Co-commit a `shard_upload` usage_log row inside the shard-insert tx
///. Failure aborts the tx — acceptable, documented in module docs.
pub async fn record_shard_upload_tx(
    tx: &mut Transaction<'_, Postgres>,
    ctx: &AuthContext,
    shard_hash: &[u8; 32],
    bytes: i64,
) -> Result<(), sqlx::Error> {
    usage_log::insert_shard_upload_tx(tx, ctx.api_key_id, ctx.user_id, shard_hash, bytes).await
}

/// Record a reconstruction query hit. `bytes` is intentionally not taken
/// reconstruction is a metadata read with no payload. gateway will
/// meter actual bytes-served via `event='xorb_serve'` rows.
pub async fn record_reconstruction(
    pool: &PgPool,
    ctx: &AuthContext,
    file_id: &[u8; 32],
) -> Result<(), sqlx::Error> {
    usage_log::insert_reconstruction(pool, ctx.api_key_id, ctx.user_id, file_id).await
}

/// Convenience wrapper that converts a sqlx error to a `tracing::warn!` log
/// line and swallows it. Handlers call this on best-effort paths:
/// ```ignore
/// metering::log_on_err(
/// "usage_log insert (xorb_upload) failed",
/// metering::record_xorb_upload(pool, &ctx, &hash, bytes).await,
/// );
/// ```
pub fn log_on_err(what: &'static str, res: Result<(), sqlx::Error>) {
    if let Err(e) = res {
        tracing::warn!(err = %e, "{}", what);
    }
}
