//! usage_log table queries (Plan 02-09 — OPS-05 / D-17).
//!
//! Synchronous single-row INSERT helpers called from Plan 02-04 (xorb upload),
//! 02-05 (shard upload), and 02-06 (V1 + batch reconstruction). Rationale:
//! CONTEXT D-17 (latency is 1–3 ms noise-floor against 100s-of-ms Sia calls;
//! no channel/batcher → no lost-events-on-crash; Phase 4 CONSOLE-03..08 reads
//! these rows directly with zero forward-migration).
//!
//! Schema reference: `cas/migrations/0003_usage_log_oauth.sql`. `event` is a
//! Postgres enum (`usage_event`); `cache_hit` is nullable and left NULL by all
//! Phase 2 handlers — Phase 3 gateway is the sole writer that sets
//! `cache_hit=Some(_)` (D-19 items 1+2).
//!
//! Each helper has two variants:
//!   * `insert_*` — writes against a `&PgPool` (handler runs outside a tx).
//!   * `insert_*_tx` — writes inside an existing `&mut Transaction` so the
//!     row co-commits with the handler's primary write (Plan 02-05 shard
//!     upload uses this variant).
//!
//! The two-variant split (rather than a generic `impl Executor` bound) is a
//! deliberate concession to sqlx 0.8.6 ergonomics — passing an
//! `impl Executor<'_, Database = Postgres>` through `sqlx::query(...).execute`
//! compiles but triggers clippy lifetime warnings at the handler call-site
//! because `&mut *tx` re-borrows each time. The explicit variants compile
//! cleanly and keep tokens predictable.

use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Pool variants — used from `handlers::xorbs` (xorb upload, post-commit) and
// `handlers::reconstruction` (V1 + batch reconstruction, read-only handler).
// ---------------------------------------------------------------------------

/// Append `event='xorb_upload'` row. Called by Plan 02-04 after `set_pin_state
/// (Pinned, ...)` succeeds AND on the dedup (was_inserted=false) path so both
/// branches record the upload against the API key.
pub async fn insert_xorb_upload(
    pool: &PgPool,
    api_key_id: Uuid,
    user_id: i64,
    xorb_hash: &[u8; 32],
    bytes: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO usage_log (event, api_key_id, user_id, xorb_hash, bytes) \
         VALUES ('xorb_upload'::usage_event, $1, $2, $3, $4)",
    )
    .bind(api_key_id)
    .bind(user_id)
    .bind(&xorb_hash[..])
    .bind(bytes)
    .execute(pool)
    .await
    .map(|_| ())
}

/// Append `event='shard_upload'` row against a pool (best-effort, post-commit
/// call from Plan 02-05 on Sia success + dedup branches).
pub async fn insert_shard_upload(
    pool: &PgPool,
    api_key_id: Uuid,
    user_id: i64,
    shard_hash: &[u8; 32],
    bytes: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO usage_log (event, api_key_id, user_id, shard_hash, bytes) \
         VALUES ('shard_upload'::usage_event, $1, $2, $3, $4)",
    )
    .bind(api_key_id)
    .bind(user_id)
    .bind(&shard_hash[..])
    .bind(bytes)
    .execute(pool)
    .await
    .map(|_| ())
}

/// Append `event='reconstruction'` row. `bytes` is NULL — reconstruction is a
/// metadata query. Phase 3 gateway's `event='xorb_serve'` rows will carry the
/// actual bytes-served counter.
///
/// `file_id` is stored under the `file_id` column (D-19 schema has a dedicated
/// nullable `file_id BYTEA` column — we use it rather than re-purposing
/// `xorb_hash`).
pub async fn insert_reconstruction(
    pool: &PgPool,
    api_key_id: Uuid,
    user_id: i64,
    file_id: &[u8; 32],
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO usage_log (event, api_key_id, user_id, file_id) \
         VALUES ('reconstruction'::usage_event, $1, $2, $3)",
    )
    .bind(api_key_id)
    .bind(user_id)
    .bind(&file_id[..])
    .execute(pool)
    .await
    .map(|_| ())
}

// ---------------------------------------------------------------------------
// Transaction variants — co-commit with the handler's primary row write.
// ---------------------------------------------------------------------------

/// Append `event='xorb_upload'` row inside an existing transaction. Reserved
/// for a future upgrade path where the xorb handler co-commits metering with
/// the `insert_pending` row write (currently post-commit per T-02-09-05).
pub async fn insert_xorb_upload_tx(
    tx: &mut Transaction<'_, Postgres>,
    api_key_id: Uuid,
    user_id: i64,
    xorb_hash: &[u8; 32],
    bytes: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO usage_log (event, api_key_id, user_id, xorb_hash, bytes) \
         VALUES ('xorb_upload'::usage_event, $1, $2, $3, $4)",
    )
    .bind(api_key_id)
    .bind(user_id)
    .bind(&xorb_hash[..])
    .bind(bytes)
    .execute(&mut **tx)
    .await
    .map(|_| ())
}

/// Append `event='shard_upload'` row inside the shard insert transaction so
/// the usage_log row co-commits with the `shards` + `reconstruction_*` rows.
/// Plan 02-05's handler uses this variant inside its BEGIN/COMMIT block.
///
/// A failure here aborts the tx — acceptable per D-17 / plan Task 1
/// ("the whole state is corrupt; better to fail fast"). The shard handler
/// already bubbles the error through `AppError::Db`.
pub async fn insert_shard_upload_tx(
    tx: &mut Transaction<'_, Postgres>,
    api_key_id: Uuid,
    user_id: i64,
    shard_hash: &[u8; 32],
    bytes: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO usage_log (event, api_key_id, user_id, shard_hash, bytes) \
         VALUES ('shard_upload'::usage_event, $1, $2, $3, $4)",
    )
    .bind(api_key_id)
    .bind(user_id)
    .bind(&shard_hash[..])
    .bind(bytes)
    .execute(&mut **tx)
    .await
    .map(|_| ())
}

/// Append `event='reconstruction'` row inside a transaction. Reserved for a
/// future reconstruction-batch pipeline that opens a tx; the Phase 2 handler
/// uses the pool variant because no other tx is open on that path.
pub async fn insert_reconstruction_tx(
    tx: &mut Transaction<'_, Postgres>,
    api_key_id: Uuid,
    user_id: i64,
    file_id: &[u8; 32],
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO usage_log (event, api_key_id, user_id, file_id) \
         VALUES ('reconstruction'::usage_event, $1, $2, $3)",
    )
    .bind(api_key_id)
    .bind(user_id)
    .bind(&file_id[..])
    .execute(&mut **tx)
    .await
    .map(|_| ())
}
