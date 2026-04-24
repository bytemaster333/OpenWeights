//! Task 5 — reconciler boundary tests (Postgres-free).
//! The full DB-backed reconciler sweep tests (reconcile_once against seeded
//! xorbs/shards rows) live in 's conformance crate where
//! testcontainers Postgres is already wired. This file asserts the
//! boundary-layer invariants that do NOT need a live database:
//! * The `ReconcilerMetrics` trait contract is honored by the real
//! `Metrics` struct (each method bumps the expected series exactly once).
//! * The `NoopMetrics` helper in the storage crate is a true no-op.
//! * The `MAX_PIN_ATTEMPTS` / `RECONCILER_TICK` / `STUCK_THRESHOLD`
//! constants match the policy (so a silent change would trip a
//! review).
//! When lands, it adds the five DB-backed tests from the plan
//! (pinning→pinned, orphaned-after-5, skip-recent, idempotent double-run,
//! shard mirror).

use std::sync::Arc;

use crate::metrics::Metrics;
use siahub_cas_storage::reconciler::{
    MAX_PIN_ATTEMPTS, NoopMetrics, RECONCILER_BATCH, RECONCILER_TICK, ReconcilerMetrics,
    STUCK_THRESHOLD,
};

// ---------------------------------------------------------------------------
// policy constants — silent change → review prompt via test diff.
// ---------------------------------------------------------------------------

#[test]
fn reconciler_policy_constants_match_d15() {
    assert_eq!(MAX_PIN_ATTEMPTS, 5, "D-15 caps attempts at 5");
    assert_eq!(
        RECONCILER_TICK.as_secs(),
        60,
        "D-15 ticks every 60 seconds"
    );
    assert_eq!(STUCK_THRESHOLD, "5 minutes", "D-15 stuck window is 5 minutes");
    assert_eq!(
        RECONCILER_BATCH, 20,
        "plan Task 3 caps sweeps at 20 rows per tick"
    );
}

// ---------------------------------------------------------------------------
// ReconcilerMetrics trait adapter — siahub-cas-core::Metrics bridges the
// cross-crate boundary with trait inversion (storage does NOT depend on core).
// ---------------------------------------------------------------------------

#[test]
fn metrics_impls_reconciler_boundary_trait() {
    let m = Metrics::new();
    // Sweep, failure, orphaned-xorb, orphaned-shard each map to a distinct
    // counter. A copy-paste bug (e.g. inc_sweep bumping failure counter)
    // trips here.
    ReconcilerMetrics::inc_sweep(&m);
    assert_eq!(m.reconciler_sweeps_total.get(), 1);
    assert_eq!(m.reconciler_failures_total.get(), 0);

    ReconcilerMetrics::inc_failure(&m);
    assert_eq!(m.reconciler_failures_total.get(), 1);

    ReconcilerMetrics::inc_orphaned_xorb(&m);
    assert_eq!(m.xorb_orphaned_total.get(), 1);
    assert_eq!(m.shard_orphaned_total.get(), 0);

    ReconcilerMetrics::inc_orphaned_shard(&m);
    assert_eq!(m.shard_orphaned_total.get(), 1);
    assert_eq!(m.xorb_orphaned_total.get(), 1, "shard bump must not touch xorb series");
}

#[test]
fn noop_metrics_never_panics_under_trait_dispatch() {
    // If anybody swaps in NoopMetrics (conformance crate tests may), the
    // reconciler must not behave any differently beyond dropping counts.
    let n: Arc<NoopMetrics> = Arc::new(NoopMetrics);
    ReconcilerMetrics::inc_sweep(&*n);
    ReconcilerMetrics::inc_failure(&*n);
    ReconcilerMetrics::inc_orphaned_xorb(&*n);
    ReconcilerMetrics::inc_orphaned_shard(&*n);
}

#[test]
fn metrics_orphaned_series_names_are_stable() {
    // Ops alerting keys on the exact series names (Plan Task 2 /
    // "alert on >0"). Any typo in Metrics::new would make this test
    // surface the drift by text search instead of at alert-time.
    let m = Metrics::new();
    m.xorb_orphaned_total.inc();
    m.shard_orphaned_total.inc();
    m.reconciler_failures_total.inc_by(2);
    m.reconciler_sweeps_total.inc_by(7);
    let s = m.encode();
    assert!(
        s.contains("siahub_cas_xorb_orphaned_total"),
        "xorb orphaned series name"
    );
    assert!(
        s.contains("siahub_cas_shard_orphaned_total"),
        "shard orphaned series name"
    );
    assert!(
        s.contains("siahub_cas_reconciler_sweeps_total"),
        "reconciler sweeps series name"
    );
    assert!(
        s.contains("siahub_cas_reconciler_failures_total"),
        "reconciler failures series name"
    );
}
