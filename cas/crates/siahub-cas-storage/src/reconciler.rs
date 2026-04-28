//! Pin-state reconciler.
//! ## What this module does
//! A background Tokio task ticks every 60 s; on each tick it scans `xorbs`
//! and `shards` for rows stuck in `pin_state IN ('uploading', 'pinning')`
//! whose `last_pin_attempt_at` is older than 5 min (or NULL, which means
//! "never attempted after insert"), and retries the Sia call that would
//! advance them toward `'pinned'`. After 5 failed attempts the row is
//! transitioned to `'orphaned'` and the matching Prometheus counter is
//! bumped so ops can alert on it.
//! ## `'uploading'` post-crash policy (documented v1 behavior)
//! A handler that crashes between `INSERT pin_state='pinning'` and the Sia
//! SDK call has no way to recover the bytes — the xorb body is in-flight to
//! Sia, but the row never received a `sia_object_id`. For xorb uploads the
//! schema default is `'pinning'` ( / migration 0002), not
//! `'uploading'`, so this case is extremely narrow. But if a row is ever
//! observed in `pin_state='uploading'` by the reconciler, it is transitioned
//! DIRECTLY to `'orphaned'` on the first sweep — we cannot re-run
//! `sdk.upload(bytes)` without the bytes. Ops intervention required.
//! ## Idempotency (T-02- mitigation)
//! The reconciler's UPDATE is not wrapped in `SELECT... FOR UPDATE`. If a
//! handler wins a race and flips a row to `'pinned'` between our SELECT and
//! UPDATE, our UPDATE still fires and bumps `pin_attempts` — but because
//! `set_pin_state` uses `COALESCE`, it does NOT overwrite the `sia_object_id`
//! the handler just wrote. Re-running `pin_only` against an already-pinned
//! object is a no-op at the Sia layer. Worst-case: pin_attempts counter
//! drifts by at most 1.
//! ## No cycle with siahub-cas-core
//! This module defines the [`ReconcilerMetrics`] trait locally and takes it
//! as a generic. `siahub-cas-core`'s `Metrics` struct implements this trait.
//! If we imported `Metrics` directly we would form a dependency cycle
//! (`storage -> core -> storage` via `SiaAdapter`). Trait inversion is the
//! standard fix — see plan Task 3's "avoid cycle" note.

use std::sync::Arc;
use std::time::Duration;

use sqlx::PgPool;

use crate::sia::{SiaAdapter, SiaAdapterError};

/// Tick interval for the reconciler loop ( policy).
pub const RECONCILER_TICK: Duration = Duration::from_secs(60);

/// Upper bound on rows swept per tick. Keeps a runaway reconciler from
/// monopolizing a Sia connection; ops surfaces persist between ticks so
/// nothing is lost.
pub const RECONCILER_BATCH: i64 = 20;

/// Per-row attempt cap before a row is transitioned to `'orphaned'`.
pub const MAX_PIN_ATTEMPTS: i32 = 5;

/// Rows are only re-attempted if `last_pin_attempt_at < NOW - this` (or NULL).
/// Prevents a tight loop when a Sia partition lasts minutes.
pub const STUCK_THRESHOLD: &str = "5 minutes";

/// Outbound metrics signal for the reconciler.
/// Implemented by `siahub_cas_core::metrics::Metrics` — the two crates meet
/// here so storage does not import core.
/// All four methods are `&self` + no-fail: these are pure counter bumps.
pub trait ReconcilerMetrics: Send + Sync + 'static {
    fn inc_sweep(&self);
    fn inc_failure(&self);
    fn inc_orphaned_xorb(&self);
    fn inc_orphaned_shard(&self);
}

/// No-op metrics impl — useful for unit tests that want to exercise the
/// reconciler without pulling in the real `Metrics` struct.
pub struct NoopMetrics;

impl ReconcilerMetrics for NoopMetrics {
    fn inc_sweep(&self) {}
    fn inc_failure(&self) {}
    fn inc_orphaned_xorb(&self) {}
    fn inc_orphaned_shard(&self) {}
}

/// One row under reconciliation. Kept flat (no Option<sia_object_id> vs
/// separate ``XorbRow`` / ``ShardRow`` split) so the sweep helpers compose.
#[derive(Debug, Clone)]
struct StuckRow {
    hash: [u8; 32],
    sia_object_id: Option<Vec<u8>>,
    pin_state: String,
    pin_attempts: i32,
}

/// Tuple shape returned by the stuck-row SELECT queries. Hoisted into a type
/// alias so clippy's `type_complexity` lint stays green without a per-site
/// `#[allow]` attribute (the tuple ordering here IS load-bearing — columns
/// match the SELECT list verbatim, and a rename would break silently).
type StuckRowTuple = (Vec<u8>, Option<Vec<u8>>, String, i32);

/// Long-running reconciler loop. Call from `main.rs::run` via
/// `tokio::spawn(run_reconciler(...))`. Returns only if the pool is
/// permanently unusable (we never intentionally exit).
pub async fn run_reconciler<M>(pool: PgPool, sia: Arc<dyn SiaAdapter>, metrics: Arc<M>)
where
    M: ReconcilerMetrics,
{
    let mut ticker = tokio::time::interval(RECONCILER_TICK);
    // Skip the immediate-fire initial tick — boot sequence handles the first
    // sweep on its own schedule.
    ticker.tick().await;
    loop {
        ticker.tick().await;
        match reconcile_once(&pool, sia.as_ref(), metrics.as_ref()).await {
            Ok(0) => {}
            Ok(n) => tracing::info!(reconciled = n, "pin reconciler sweep handled rows"),
            Err(e) => {
                tracing::error!(err = %e, "reconciler sweep error");
                metrics.inc_failure();
            }
        }
    }
}

/// Execute one reconciliation pass. Returns the number of rows whose state
/// was advanced or orphaned. Zero means "nothing to do" — the caller uses
/// this to gate the `inc_sweep` counter.
pub async fn reconcile_once<M>(
    pool: &PgPool,
    sia: &dyn SiaAdapter,
    metrics: &M,
) -> anyhow::Result<usize>
where
    M: ReconcilerMetrics,
{
    let mut handled = 0usize;

    // (A) Xorbs sweep.
    let xorb_rows = fetch_stuck_xorbs(pool).await?;
    for row in xorb_rows {
        match reconcile_xorb_row(pool, sia, metrics, &row).await {
            Ok(true) => handled += 1,
            Ok(false) => {}
            Err(e) => {
                tracing::warn!(err = %e, hash = %hex(&row.hash), "reconcile xorb row failed");
            }
        }
    }

    // (B) Shards sweep.
    let shard_rows = fetch_stuck_shards(pool).await?;
    for row in shard_rows {
        match reconcile_shard_row(pool, sia, metrics, &row).await {
            Ok(true) => handled += 1,
            Ok(false) => {}
            Err(e) => {
                tracing::warn!(err = %e, hash = %hex(&row.hash), "reconcile shard row failed");
            }
        }
    }

    if handled > 0 {
        metrics.inc_sweep();
    }
    Ok(handled)
}

async fn fetch_stuck_xorbs(pool: &PgPool) -> Result<Vec<StuckRow>, sqlx::Error> {
    // END-EXCLUSIVE `pin_attempts < 5` keeps rows that already hit 5 out of
    // the retry loop. Threshold is a string literal because Postgres can't
    // parametrize INTERVAL fragments cleanly.
    let q = format!(
        "SELECT xorb_merkle_hash, sia_object_id, pin_state::text, pin_attempts \
         FROM xorbs \
         WHERE pin_state IN ('uploading', 'pinning') \
           AND (last_pin_attempt_at IS NULL \
                OR last_pin_attempt_at < NOW() - INTERVAL '{STUCK_THRESHOLD}') \
           AND pin_attempts < {MAX_PIN_ATTEMPTS} \
         ORDER BY last_pin_attempt_at ASC NULLS FIRST \
         LIMIT {RECONCILER_BATCH}",
    );
    let rows: Vec<StuckRowTuple> = sqlx::query_as(&q).fetch_all(pool).await?;
    Ok(rows.into_iter().map(|(h, id, state, att)| {
        let mut arr = [0u8; 32];
        if h.len() == 32 {
            arr.copy_from_slice(&h);
        }
        StuckRow { hash: arr, sia_object_id: id, pin_state: state, pin_attempts: att }
    }).collect())
}

async fn fetch_stuck_shards(pool: &PgPool) -> Result<Vec<StuckRow>, sqlx::Error> {
    let q = format!(
        "SELECT shard_hash, sia_object_id, pin_state::text, pin_attempts \
         FROM shards \
         WHERE pin_state IN ('uploading', 'pinning') \
           AND (last_pin_attempt_at IS NULL \
                OR last_pin_attempt_at < NOW() - INTERVAL '{STUCK_THRESHOLD}') \
           AND pin_attempts < {MAX_PIN_ATTEMPTS} \
         ORDER BY last_pin_attempt_at ASC NULLS FIRST \
         LIMIT {RECONCILER_BATCH}",
    );
    let rows: Vec<StuckRowTuple> = sqlx::query_as(&q).fetch_all(pool).await?;
    Ok(rows.into_iter().map(|(h, id, state, att)| {
        let mut arr = [0u8; 32];
        if h.len() == 32 {
            arr.copy_from_slice(&h);
        }
        StuckRow { hash: arr, sia_object_id: id, pin_state: state, pin_attempts: att }
    }).collect())
}

async fn reconcile_xorb_row<M>(
    pool: &PgPool,
    sia: &dyn SiaAdapter,
    metrics: &M,
    row: &StuckRow,
) -> anyhow::Result<bool>
where
    M: ReconcilerMetrics,
{
    // 'uploading' state after crash: bytes unavailable → orphan on first
    // sweep (see module docs for the policy rationale).
    if row.pin_state == "uploading" {
        orphan_xorb(pool, &row.hash).await?;
        metrics.inc_orphaned_xorb();
        return Ok(true);
    }

    // 'pinning' state, two paths:
    // (a) sia_object_id is set → handler successfully uploaded but the host
    //     pin step failed; retry `pin_only` (cheap, idempotent).
    // (b) sia_object_id is NULL → handler hit `Sia unavailable` on the
    //     initial upload (e.g. wallet was empty when uploads started) and
    //     left the row pending. Bytes are durable in `xorb_bodies` — load
    //     them and re-run `upload_and_pin` from the reconciler so the row
    //     can finally reach Sia. This is the recovery path for "the demo
    //     ran without a funded wallet, then the wallet got funded".
    match row.sia_object_id.as_deref() {
        Some(oid) => match sia.pin_only(oid).await {
            Ok(()) => {
                set_pinned_xorb(pool, &row.hash, oid).await?;
                Ok(true)
            }
            Err(e) => bump_xorb_attempt(pool, &row.hash, row.pin_attempts, metrics, &e).await,
        },
        None => match load_xorb_body(pool, &row.hash).await? {
            Some(bytes) => match sia.upload_and_pin(&bytes).await {
                Ok(sia_id) => {
                    set_pinned_xorb(pool, &row.hash, &sia_id).await?;
                    Ok(true)
                }
                Err(e) => bump_xorb_attempt(pool, &row.hash, row.pin_attempts, metrics, &e).await,
            },
            None => {
                // Bytes truly gone — can't recover. Bump so the row eventually orphans.
                let noent = SiaAdapterError::Other(anyhow::anyhow!(
                    "pin_state='pinning' but xorb_bodies row is missing"
                ));
                bump_xorb_attempt(pool, &row.hash, row.pin_attempts, metrics, &noent).await
            }
        },
    }
}

/// Load the raw xorb body from the durable Postgres cache. Returns None if
/// the row is missing (which only happens after a manual purge — handlers
/// always co-insert the body alongside the metadata row inside one txn).
async fn load_xorb_body(pool: &PgPool, hash: &[u8; 32]) -> Result<Option<Vec<u8>>, sqlx::Error> {
    let row: Option<(Vec<u8>,)> = sqlx::query_as(
        "SELECT content FROM xorb_bodies WHERE xorb_hash = $1",
    )
    .bind(&hash[..])
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(b,)| b))
}

async fn reconcile_shard_row<M>(
    pool: &PgPool,
    sia: &dyn SiaAdapter,
    metrics: &M,
    row: &StuckRow,
) -> anyhow::Result<bool>
where
    M: ReconcilerMetrics,
{
    if row.pin_state == "uploading" {
        orphan_shard(pool, &row.hash).await?;
        metrics.inc_orphaned_shard();
        return Ok(true);
    }
    match row.sia_object_id.as_deref() {
        Some(oid) => match sia.pin_only(oid).await {
            Ok(()) => {
                set_pinned_shard(pool, &row.hash, oid).await?;
                Ok(true)
            }
            Err(e) => bump_shard_attempt(pool, &row.hash, row.pin_attempts, metrics, &e).await,
        },
        None => {
            let noent = anyhow::anyhow!("pin_state='pinning' but sia_object_id is NULL");
            bump_shard_attempt(pool, &row.hash, row.pin_attempts, metrics, &SiaAdapterError::Other(noent)).await
        }
    }
}

/// UPDATE xorbs row to `pin_state='pinned'` with the resolved id. Does NOT
/// bump pin_attempts — successful transitions record the actual outcome.
async fn set_pinned_xorb(pool: &PgPool, hash: &[u8; 32], sia_id: &[u8]) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE xorbs \
         SET pin_state = 'pinned'::xorb_pin_state, \
             sia_object_id = COALESCE(sia_object_id, $1), \
             last_pin_attempt_at = NOW() \
         WHERE xorb_merkle_hash = $2 AND pin_state <> 'pinned'",
    )
    .bind(sia_id)
    .bind(&hash[..])
    .execute(pool)
    .await?;
    Ok(())
}

async fn set_pinned_shard(pool: &PgPool, hash: &[u8; 32], sia_id: &[u8]) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE shards \
         SET pin_state = 'pinned'::xorb_pin_state, \
             sia_object_id = COALESCE(sia_object_id, $1), \
             last_pin_attempt_at = NOW() \
         WHERE shard_hash = $2 AND pin_state <> 'pinned'",
    )
    .bind(sia_id)
    .bind(&hash[..])
    .execute(pool)
    .await?;
    Ok(())
}

async fn bump_xorb_attempt<M>(
    pool: &PgPool,
    hash: &[u8; 32],
    prior_attempts: i32,
    metrics: &M,
    err: &SiaAdapterError,
) -> anyhow::Result<bool>
where
    M: ReconcilerMetrics,
{
    let new_attempts = prior_attempts + 1;
    if new_attempts >= MAX_PIN_ATTEMPTS {
        orphan_xorb(pool, hash).await?;
        metrics.inc_orphaned_xorb();
        tracing::warn!(err = %err, hash = %hex(hash), attempts = new_attempts, "xorb orphaned after failed pin attempts");
    } else {
        sqlx::query(
            "UPDATE xorbs \
             SET pin_attempts = pin_attempts + 1, \
                 last_pin_attempt_at = NOW() \
             WHERE xorb_merkle_hash = $1",
        )
        .bind(&hash[..])
        .execute(pool)
        .await?;
        tracing::warn!(err = %err, hash = %hex(hash), attempts = new_attempts, "xorb pin retry failed; will sweep again");
    }
    Ok(true)
}

async fn bump_shard_attempt<M>(
    pool: &PgPool,
    hash: &[u8; 32],
    prior_attempts: i32,
    metrics: &M,
    err: &SiaAdapterError,
) -> anyhow::Result<bool>
where
    M: ReconcilerMetrics,
{
    let new_attempts = prior_attempts + 1;
    if new_attempts >= MAX_PIN_ATTEMPTS {
        orphan_shard(pool, hash).await?;
        metrics.inc_orphaned_shard();
        tracing::warn!(err = %err, hash = %hex(hash), attempts = new_attempts, "shard orphaned after failed pin attempts");
    } else {
        sqlx::query(
            "UPDATE shards \
             SET pin_attempts = pin_attempts + 1, \
                 last_pin_attempt_at = NOW() \
             WHERE shard_hash = $1",
        )
        .bind(&hash[..])
        .execute(pool)
        .await?;
        tracing::debug!(err = %err, hash = %hex(hash), attempts = new_attempts, "shard pin retry failed; will sweep again");
    }
    Ok(true)
}

async fn orphan_xorb(pool: &PgPool, hash: &[u8; 32]) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE xorbs \
         SET pin_state = 'orphaned'::xorb_pin_state, \
             pin_attempts = pin_attempts + 1, \
             last_pin_attempt_at = NOW() \
         WHERE xorb_merkle_hash = $1",
    )
    .bind(&hash[..])
    .execute(pool)
    .await?;
    Ok(())
}

async fn orphan_shard(pool: &PgPool, hash: &[u8; 32]) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE shards \
         SET pin_state = 'orphaned'::xorb_pin_state, \
             pin_attempts = pin_attempts + 1, \
             last_pin_attempt_at = NOW() \
         WHERE shard_hash = $1",
    )
    .bind(&hash[..])
    .execute(pool)
    .await?;
    Ok(())
}

/// Log-safe lowercase hex of a 32-byte hash. Only used in tracing fields;
/// NEVER on the wire (.md §1 — use MerkleHash::hex for wire strings).
fn hex(h: &[u8; 32]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(64);
    for b in h {
        let _ = write!(s, "{b:02x}");
    }
    s
}
