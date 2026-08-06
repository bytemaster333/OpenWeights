//! db-backed sweep tests for the pin reconciler.
//!
//! these drive `reconcile_once` end to end against a throwaway postgres
//! (testcontainers) with the real migrations and the in-memory MockSiaAdapter,
//! covering the state machine both handlers rely on the reconciler to finish:
//! pinning -> pinned, null-sia-id body recovery, uploading -> orphaned, the
//! transient-vs-permanent orphan-cap split, the 5-minute staleness skip, the
//! per-tick batch cap, and sweep-counter gating -- for xorbs and shards.
//!
//! docker is required. when it is absent each test prints a SKIP line and
//! returns early instead of failing, matching the conformance harness.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use sha2::{Digest, Sha256};
use sqlx::PgPool;
use sqlx::types::Uuid;

use crate::mock::MockSiaAdapter;
use crate::reconciler::{RECONCILER_BATCH, ReconcilerMetrics, reconcile_once};
use crate::sia::SiaAdapter;

// ---------------------------------------------------------------------------
// metrics spy — records how many times each counter would have been bumped.
// ---------------------------------------------------------------------------

#[derive(Default)]
struct CountingMetrics {
    sweeps: AtomicUsize,
    failures: AtomicUsize,
    orphaned_xorb: AtomicUsize,
    orphaned_shard: AtomicUsize,
}

impl ReconcilerMetrics for CountingMetrics {
    fn inc_sweep(&self) {
        self.sweeps.fetch_add(1, Ordering::SeqCst);
    }
    fn inc_failure(&self) {
        self.failures.fetch_add(1, Ordering::SeqCst);
    }
    fn inc_orphaned_xorb(&self) {
        self.orphaned_xorb.fetch_add(1, Ordering::SeqCst);
    }
    fn inc_orphaned_shard(&self) {
        self.orphaned_shard.fetch_add(1, Ordering::SeqCst);
    }
}

impl CountingMetrics {
    fn sweeps(&self) -> usize {
        self.sweeps.load(Ordering::SeqCst)
    }
    fn failures(&self) -> usize {
        self.failures.load(Ordering::SeqCst)
    }
    fn orphaned_xorb(&self) -> usize {
        self.orphaned_xorb.load(Ordering::SeqCst)
    }
    fn orphaned_shard(&self) -> usize {
        self.orphaned_shard.load(Ordering::SeqCst)
    }
}

// ---------------------------------------------------------------------------
// postgres harness
// ---------------------------------------------------------------------------

type PgNode = testcontainers::ContainerAsync<testcontainers_modules::postgres::Postgres>;

/// bring up a throwaway postgres, apply the full migration set, hand back a
/// pool. returns None (and prints a SKIP line) when docker is unreachable so
/// the suite degrades to a no-op instead of a hard failure.
async fn try_pg() -> Option<(PgPool, PgNode)> {
    use testcontainers::core::ImageExt;
    use testcontainers::runners::AsyncRunner;
    let node = match testcontainers_modules::postgres::Postgres::default()
        .with_db_name("openweights")
        .with_user("openweights")
        .with_password("test-pw")
        // prod is postgres 17; the migrations use pg12+ generated columns, so
        // pin a modern tag instead of the module's older default.
        .with_tag("17-alpine")
        .start()
        .await
    {
        Ok(n) => n,
        Err(e) => {
            eprintln!("SKIP reconciler db tests: docker/testcontainers unavailable: {e}");
            return None;
        }
    };
    let port = node.get_host_port_ipv4(5432).await.ok()?;
    let url = format!("postgres://openweights:test-pw@127.0.0.1:{port}/openweights");
    let pool = connect_with_retry(&url).await?;

    // migration 0005 hard-requires the openweights_gw role to already exist (in
    // prod it is created by ops/indexd-postgres-init.sql). create it here so the
    // full migration set applies against a bare container.
    sqlx::query(
        "DO $$ BEGIN \
           IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'openweights_gw') THEN \
             CREATE ROLE openweights_gw LOGIN PASSWORD 'test-pw'; \
           END IF; \
         END $$;",
    )
    .execute(&pool)
    .await
    .expect("create openweights_gw role");

    sqlx::migrate!("../../migrations")
        .run(&pool)
        .await
        .expect("apply migrations");
    Some((pool, node))
}

async fn connect_with_retry(url: &str) -> Option<PgPool> {
    for _ in 0..12 {
        match sqlx::postgres::PgPoolOptions::new()
            .max_connections(4)
            .connect(url)
            .await
        {
            Ok(p) => return Some(p),
            Err(_) => tokio::time::sleep(Duration::from_millis(500)).await,
        }
    }
    eprintln!("SKIP reconciler db tests: postgres never became reachable");
    None
}

// ---------------------------------------------------------------------------
// seed + read helpers
// ---------------------------------------------------------------------------

/// last_pin_attempt_at seeding: `Never` (NULL) is eligible immediately;
/// `Recent` (NOW()) sits inside the 5-minute window and must be skipped.
#[derive(Clone, Copy)]
enum Last {
    Never,
    Recent,
}

fn last_sql(l: Last) -> &'static str {
    match l {
        Last::Never => "NULL",
        Last::Recent => "NOW()",
    }
}

fn hx(n: u8) -> [u8; 32] {
    [n; 32]
}

/// one owner (user 1 + a single api key) satisfies the xorbs/shards FKs.
async fn seed_owner(pool: &PgPool) -> Uuid {
    sqlx::query("INSERT INTO users (id, github_login) VALUES (1, 'tester')")
        .execute(pool)
        .await
        .expect("seed user");
    sqlx::query_scalar(
        "INSERT INTO api_keys (user_id, key_hash, scopes) \
         VALUES (1, $1, ARRAY['upload','download']::api_key_scope[]) RETURNING id",
    )
    .bind(&[7u8; 32][..])
    .fetch_one(pool)
    .await
    .expect("seed api key")
}

async fn seed_xorb(
    pool: &PgPool,
    key: Uuid,
    hash: &[u8; 32],
    sia_id: Option<&[u8]>,
    state: &str,
    attempts: i32,
    last: Last,
) {
    let q = format!(
        "INSERT INTO xorbs \
           (xorb_merkle_hash, sia_object_id, size_bytes, owner_user_id, owner_api_key_id, \
            pin_state, pin_attempts, last_pin_attempt_at) \
         VALUES ($1, $2, 10, 1, $3, $4::xorb_pin_state, $5, {})",
        last_sql(last)
    );
    sqlx::query(&q)
        .bind(&hash[..])
        .bind(sia_id)
        .bind(key)
        .bind(state)
        .bind(attempts)
        .execute(pool)
        .await
        .expect("seed xorb");
}

async fn seed_shard(
    pool: &PgPool,
    key: Uuid,
    hash: &[u8; 32],
    sia_id: Option<&[u8]>,
    state: &str,
    attempts: i32,
    last: Last,
) {
    let q = format!(
        "INSERT INTO shards \
           (shard_hash, sia_object_id, size_bytes, owner_user_id, owner_api_key_id, \
            pin_state, pin_attempts, last_pin_attempt_at) \
         VALUES ($1, $2, 10, 1, $3, $4::xorb_pin_state, $5, {})",
        last_sql(last)
    );
    sqlx::query(&q)
        .bind(&hash[..])
        .bind(sia_id)
        .bind(key)
        .bind(state)
        .bind(attempts)
        .execute(pool)
        .await
        .expect("seed shard");
}

async fn seed_xorb_body(pool: &PgPool, hash: &[u8; 32], content: &[u8]) {
    sqlx::query("INSERT INTO xorb_bodies (xorb_hash, content) VALUES ($1, $2)")
        .bind(&hash[..])
        .bind(content)
        .execute(pool)
        .await
        .expect("seed xorb body");
}

async fn xorb_state(pool: &PgPool, hash: &[u8; 32]) -> String {
    sqlx::query_scalar("SELECT pin_state::text FROM xorbs WHERE xorb_merkle_hash = $1")
        .bind(&hash[..])
        .fetch_one(pool)
        .await
        .expect("read xorb state")
}

async fn xorb_attempts(pool: &PgPool, hash: &[u8; 32]) -> i32 {
    sqlx::query_scalar("SELECT pin_attempts FROM xorbs WHERE xorb_merkle_hash = $1")
        .bind(&hash[..])
        .fetch_one(pool)
        .await
        .expect("read xorb attempts")
}

async fn xorb_sia_id(pool: &PgPool, hash: &[u8; 32]) -> Option<Vec<u8>> {
    sqlx::query_scalar("SELECT sia_object_id FROM xorbs WHERE xorb_merkle_hash = $1")
        .bind(&hash[..])
        .fetch_one(pool)
        .await
        .expect("read xorb sia id")
}

async fn count_xorb_state(pool: &PgPool, state: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM xorbs WHERE pin_state = $1::xorb_pin_state")
        .bind(state)
        .fetch_one(pool)
        .await
        .expect("count xorbs")
}

async fn shard_state(pool: &PgPool, hash: &[u8; 32]) -> String {
    sqlx::query_scalar("SELECT pin_state::text FROM shards WHERE shard_hash = $1")
        .bind(&hash[..])
        .fetch_one(pool)
        .await
        .expect("read shard state")
}

async fn shard_attempts(pool: &PgPool, hash: &[u8; 32]) -> i32 {
    sqlx::query_scalar("SELECT pin_attempts FROM shards WHERE shard_hash = $1")
        .bind(&hash[..])
        .fetch_one(pool)
        .await
        .expect("read shard attempts")
}

// ---------------------------------------------------------------------------
// xorb tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn xorb_healthy_sweep_advances_recovers_orphans_and_skips() {
    let Some((pool, _node)) = try_pg().await else {
        return;
    };
    let key = seed_owner(&pool).await;
    let sia = MockSiaAdapter::new();
    let m = CountingMetrics::default();

    // (1) pinning + a sia id the backend already knows -> pin_only -> pinned.
    let good_id = sia.upload_and_pin(b"already-on-sia").await.unwrap();
    seed_xorb(
        &pool,
        key,
        &hx(1),
        Some(good_id.as_slice()),
        "pinning",
        0,
        Last::Never,
    )
    .await;

    // (2) pinning + null id + a cached body -> upload_and_pin -> pinned, with
    //     the content-addressed id written back.
    seed_xorb(&pool, key, &hx(2), None, "pinning", 0, Last::Never).await;
    seed_xorb_body(&pool, &hx(2), b"recover-me").await;
    let recovered_id: [u8; 32] = Sha256::digest(b"recover-me").into();

    // (3) uploading -> orphaned on first sweep (bytes unrecoverable).
    seed_xorb(&pool, key, &hx(3), None, "uploading", 0, Last::Never).await;

    // (4) attempted just now -> inside the 5-minute window -> skipped.
    seed_xorb(&pool, key, &hx(4), None, "pinning", 0, Last::Recent).await;

    // (5) pinning + null id + no cached body -> permanent bump (not orphan yet).
    seed_xorb(&pool, key, &hx(5), None, "pinning", 0, Last::Never).await;

    let handled = reconcile_once(&pool, &sia, &m).await.unwrap();

    assert_eq!(xorb_state(&pool, &hx(1)).await, "pinned");
    assert_eq!(xorb_state(&pool, &hx(2)).await, "pinned");
    assert_eq!(
        xorb_sia_id(&pool, &hx(2)).await.as_deref(),
        Some(&recovered_id[..]),
        "recovered id is the content hash of the cached body"
    );
    assert_eq!(xorb_state(&pool, &hx(3)).await, "orphaned");
    assert_eq!(
        xorb_state(&pool, &hx(4)).await,
        "pinning",
        "recently-attempted row must be skipped"
    );
    assert_eq!(
        xorb_attempts(&pool, &hx(4)).await,
        0,
        "skipped row is untouched"
    );
    assert_eq!(xorb_state(&pool, &hx(5)).await, "pinning");
    assert_eq!(
        xorb_attempts(&pool, &hx(5)).await,
        1,
        "missing body is a permanent bump"
    );

    assert_eq!(handled, 4, "rows 1,2,3,5 handled; 4 skipped");
    assert_eq!(m.orphaned_xorb(), 1, "only the uploading row orphaned");
    assert_eq!(m.orphaned_shard(), 0);
    assert_eq!(m.failures(), 0);
    assert_eq!(
        m.sweeps(),
        1,
        "sweep counter bumped once when work happened"
    );
}

#[tokio::test]
async fn xorb_transient_unavailable_never_orphans_or_counts() {
    let Some((pool, _node)) = try_pg().await else {
        return;
    };
    let key = seed_owner(&pool).await;
    let sia = MockSiaAdapter::new();
    sia.inject_unavailable(true);
    let m = CountingMetrics::default();

    // one attempt below the cap: a transient failure must NOT push it over.
    seed_xorb(
        &pool,
        key,
        &hx(1),
        Some(&[9u8; 32][..]),
        "pinning",
        4,
        Last::Never,
    )
    .await;

    reconcile_once(&pool, &sia, &m).await.unwrap();

    assert_eq!(
        xorb_state(&pool, &hx(1)).await,
        "pinning",
        "transient unavailability never orphans"
    );
    assert_eq!(
        xorb_attempts(&pool, &hx(1)).await,
        4,
        "transient failures never increment pin_attempts"
    );
    assert_eq!(m.orphaned_xorb(), 0);
}

#[tokio::test]
async fn xorb_permanent_failure_increments_then_orphans_at_cap() {
    let Some((pool, _node)) = try_pg().await else {
        return;
    };
    let key = seed_owner(&pool).await;
    let sia = MockSiaAdapter::new();
    sia.inject_permanent(true);
    let m = CountingMetrics::default();

    seed_xorb(
        &pool,
        key,
        &hx(1),
        Some(&[9u8; 32][..]),
        "pinning",
        0,
        Last::Never,
    )
    .await;
    seed_xorb(
        &pool,
        key,
        &hx(2),
        Some(&[9u8; 32][..]),
        "pinning",
        4,
        Last::Never,
    )
    .await;

    reconcile_once(&pool, &sia, &m).await.unwrap();

    assert_eq!(xorb_state(&pool, &hx(1)).await, "pinning");
    assert_eq!(
        xorb_attempts(&pool, &hx(1)).await,
        1,
        "a permanent failure increments the attempt counter"
    );
    assert_eq!(
        xorb_state(&pool, &hx(2)).await,
        "orphaned",
        "the 5th permanent attempt orphans the row"
    );
    assert_eq!(m.orphaned_xorb(), 1);
}

#[tokio::test]
async fn xorb_sweep_honors_batch_cap() {
    let Some((pool, _node)) = try_pg().await else {
        return;
    };
    let key = seed_owner(&pool).await;
    let sia = MockSiaAdapter::new();
    let m = CountingMetrics::default();

    // more stuck rows than the per-tick cap; uploading rows orphan without a
    // sia call, so exactly RECONCILER_BATCH of them move per sweep.
    let total = RECONCILER_BATCH as u8 + 5;
    for i in 0..total {
        seed_xorb(&pool, key, &hx(100 + i), None, "uploading", 0, Last::Never).await;
    }

    let handled = reconcile_once(&pool, &sia, &m).await.unwrap();

    assert_eq!(
        handled, RECONCILER_BATCH as usize,
        "one tick handles at most the batch cap"
    );
    assert_eq!(count_xorb_state(&pool, "orphaned").await, RECONCILER_BATCH);
    assert_eq!(
        count_xorb_state(&pool, "uploading").await,
        5,
        "the remainder waits for the next tick"
    );
}

#[tokio::test]
async fn sweep_gating_and_idempotent_double_run() {
    let Some((pool, _node)) = try_pg().await else {
        return;
    };
    let key = seed_owner(&pool).await;
    let sia = MockSiaAdapter::new();
    let m = CountingMetrics::default();

    // nothing stuck -> zero handled, and the sweep counter is NOT bumped.
    let handled0 = reconcile_once(&pool, &sia, &m).await.unwrap();
    assert_eq!(handled0, 0);
    assert_eq!(
        m.sweeps(),
        0,
        "an empty sweep does not bump the sweep counter"
    );

    // one pinnable row -> pinned, sweep counted once.
    let id = sia.upload_and_pin(b"x").await.unwrap();
    seed_xorb(
        &pool,
        key,
        &hx(1),
        Some(id.as_slice()),
        "pinning",
        0,
        Last::Never,
    )
    .await;
    let handled1 = reconcile_once(&pool, &sia, &m).await.unwrap();
    assert_eq!(handled1, 1);
    assert_eq!(xorb_state(&pool, &hx(1)).await, "pinned");
    assert_eq!(m.sweeps(), 1);

    // second run is a no-op: a pinned row is no longer eligible.
    let handled2 = reconcile_once(&pool, &sia, &m).await.unwrap();
    assert_eq!(handled2, 0, "already-pinned rows are not re-swept");
    assert_eq!(xorb_state(&pool, &hx(1)).await, "pinned");
    assert_eq!(m.sweeps(), 1, "no extra sweep bump on the idempotent run");
}

// ---------------------------------------------------------------------------
// shard tests — mirror of the xorb pin-state machine
// ---------------------------------------------------------------------------

#[tokio::test]
async fn shard_sweep_pins_and_orphans() {
    let Some((pool, _node)) = try_pg().await else {
        return;
    };
    let key = seed_owner(&pool).await;
    let sia = MockSiaAdapter::new();
    let m = CountingMetrics::default();

    let id = sia.upload_and_pin(b"shard-on-sia").await.unwrap();
    seed_shard(
        &pool,
        key,
        &hx(1),
        Some(id.as_slice()),
        "pinning",
        0,
        Last::Never,
    )
    .await;
    seed_shard(&pool, key, &hx(2), None, "uploading", 0, Last::Never).await;

    reconcile_once(&pool, &sia, &m).await.unwrap();

    assert_eq!(shard_state(&pool, &hx(1)).await, "pinned");
    assert_eq!(shard_state(&pool, &hx(2)).await, "orphaned");
    assert_eq!(m.orphaned_shard(), 1);
    assert_eq!(m.orphaned_xorb(), 0);
}

#[tokio::test]
async fn shard_transient_unavailable_never_orphans() {
    let Some((pool, _node)) = try_pg().await else {
        return;
    };
    let key = seed_owner(&pool).await;
    let sia = MockSiaAdapter::new();
    sia.inject_unavailable(true);
    let m = CountingMetrics::default();

    seed_shard(
        &pool,
        key,
        &hx(1),
        Some(&[9u8; 32][..]),
        "pinning",
        4,
        Last::Never,
    )
    .await;
    reconcile_once(&pool, &sia, &m).await.unwrap();

    assert_eq!(shard_state(&pool, &hx(1)).await, "pinning");
    assert_eq!(shard_attempts(&pool, &hx(1)).await, 4);
    assert_eq!(m.orphaned_shard(), 0);
}

#[tokio::test]
async fn shard_permanent_failure_orphans_at_cap() {
    let Some((pool, _node)) = try_pg().await else {
        return;
    };
    let key = seed_owner(&pool).await;
    let sia = MockSiaAdapter::new();
    sia.inject_permanent(true);
    let m = CountingMetrics::default();

    seed_shard(
        &pool,
        key,
        &hx(1),
        Some(&[9u8; 32][..]),
        "pinning",
        4,
        Last::Never,
    )
    .await;
    reconcile_once(&pool, &sia, &m).await.unwrap();

    assert_eq!(shard_state(&pool, &hx(1)).await, "orphaned");
    assert_eq!(m.orphaned_shard(), 1);
}
