//! usage_log table queries ( — / ).
//! Synchronous single-row INSERT helpers called from (xorb upload),
//! (shard upload), and (V1 + batch reconstruction). Rationale:
//! CONTEXT (latency is 1–3 ms noise-floor against 100s-of-ms Sia calls;
//! no channel/batcher → no lost-events-on-crash;..08 reads
//! these rows directly with zero forward-migration).
//! Schema reference: `cas/migrations/0003_usage_log_oauth.sql`. `event` is a
//! Postgres enum (`usage_event`); `cache_hit` is nullable and left NULL by all
//! handlers — gateway is the sole writer that sets
//! `cache_hit=Some(_)` ( items 1+2).
//! Each helper has two variants:
//! * `insert_*` — writes against a `&PgPool` (handler runs outside a tx).
//! * `insert_*_tx` — writes inside an existing `&mut Transaction` so the
//! row co-commits with the handler's primary write ( shard
//! upload uses this variant).
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

/// Append `event='xorb_upload'` row. Called by after `set_pin_state
/// (Pinned,...)` succeeds AND on the dedup (was_inserted=false) path so both
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
/// call from on Sia success + dedup branches).
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
/// metadata query. gateway's `event='xorb_serve'` rows will carry the
/// actual bytes-served counter.
/// `file_id` is stored under the `file_id` column ( schema has a dedicated
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
/// 's handler uses this variant inside its BEGIN/COMMIT block.
/// A failure here aborts the tx — acceptable per / plan Task 1
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
/// future reconstruction-batch pipeline that opens a tx; the handler
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
