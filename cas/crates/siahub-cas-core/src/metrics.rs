//! Prometheus registry + `/metrics` endpoint scaffold ( Task 2).
//! / : `/metrics` is mounted on the main router but the
//! Dockerfile / Compose binds the container to `127.0.0.1`, so the endpoint
//! is unreachable from the public internet by default. Caddy also
//! explicitly DENIES `/metrics` on the public vhost. The endpoint itself is
//! unauthenticated — Prometheus scrapers do not ship bearer tokens.
//! **Scope.** This module defines the `Metrics` struct, registers
//! the six counters / histograms the plan's `<interfaces>` block enumerates,
//! and exposes a single `metrics_handler` that `TextEncoder::encode`s the
//! current registry output. gateway binds its own registry; this
//! crate never reaches across the seam.
//! **Crate choice.** We use the `prometheus = "0.13"` crate (not `metrics-rs`
//! + `metrics-exporter-prometheus`) because:
//! 1. Registry is explicit — we want one owned `Registry`, not a global.
//! 2. `register_int_counter_*` returns typed handles rather than name
//! lookups, which makes compile-time ordering mistakes louder.
//! 3. The `metrics-rs` facade would force us to either make the scraper
//! also depend on it (forcing the Go gateway to round-trip through a
//! Rust facade) or split the seam in .
//! **Orphaned metric ( alert).** `xorb_orphaned_total` + its shard mirror
//! are the ops alert key: any non-zero value means a row hit 5 failed pin
//! attempts and was quarantined. See `reconciler` module for the bump site.

use std::sync::Arc;

use axum::{
    extract::State,
    http::{HeaderMap, HeaderValue, header},
    response::IntoResponse,
};
use prometheus::{
    Encoder, Histogram, HistogramOpts, IntCounter, IntCounterVec, Opts, Registry, TextEncoder,
};

use siahub_cas_storage::reconciler::ReconcilerMetrics;

/// Prometheus handle set for siahub-cas.
/// Handles are cloneable (`Counter` / `Histogram` are `Arc` internally), so
/// the handler + reconciler both hold their own pointers into the shared
/// registry without a lock.
pub struct Metrics {
    /// Owns the registry. Exposed via `encode` for the `/metrics` endpoint.
    registry: Registry,

    /// alert: xorbs that exhausted 5 pin-retry attempts. NEVER
    /// decrements; cumulative. Alert threshold > 0.
    pub xorb_orphaned_total: IntCounter,

    /// Mirror of `xorb_orphaned_total` for shards ( applies symmetrically).
    pub shard_orphaned_total: IntCounter,

    /// RESEARCH §9- — counter vec with labels `{header_version,
    /// footer_version}` so ops can see which mis-versioned clients are
    /// pounding the API. Bumped from `handlers::shards` via
    /// `metrics.shard_version_rejected_total.with_label_values(&[
    /// header_v.to_string.as_str, footer_v.to_string.as_str
    /// ]).inc`.
    pub shard_version_rejected_total: IntCounterVec,

    /// Reconciler tick counter (per successful sweep, n > 0). See plan Task 3.
    pub reconciler_sweeps_total: IntCounter,

    /// Reconciler loop counter for rows whose sweep iteration failed (e.g.,
    /// DB query error outside of a Sia call). Separate from Sia-specific
    /// pin_attempts bumps — that's a per-row column, not a counter.
    pub reconciler_failures_total: IntCounter,

    /// Sia upload duration histogram (seconds). Plan Task 2 default buckets.
    pub sia_upload_duration_seconds: Histogram,

    /// Sia pin duration histogram (seconds). Plan Task 2 default buckets.
    pub sia_pin_duration_seconds: Histogram,
}

impl Metrics {
    /// Build a fresh registry and pre-register every metric. This is
    /// called exactly once at process boot. Panics ONLY on registry-level
    /// programming errors (duplicate metric name, empty buckets, etc.) — all
    /// of which are statically-known bugs in this file rather than runtime
    /// conditions.
    pub fn new() -> Self {
        let registry = Registry::new();

        let xorb_orphaned_total = IntCounter::with_opts(Opts::new(
            "siahub_cas_xorb_orphaned_total",
            "Total xorbs transitioned to 'orphaned' pin_state after 5 failed attempts.",
        ))
        .expect("xorb_orphaned_total counter");
        registry
            .register(Box::new(xorb_orphaned_total.clone()))
            .expect("register xorb_orphaned_total");

        let shard_orphaned_total = IntCounter::with_opts(Opts::new(
            "siahub_cas_shard_orphaned_total",
            "Total shards transitioned to 'orphaned' pin_state after 5 failed attempts.",
        ))
        .expect("shard_orphaned_total counter");
        registry
            .register(Box::new(shard_orphaned_total.clone()))
            .expect("register shard_orphaned_total");

        let shard_version_rejected_total = IntCounterVec::new(
            Opts::new(
                "siahub_cas_shard_version_rejected_total",
                "Shard uploads rejected due to header/footer version mismatch (P19).",
            ),
            &["header_version", "footer_version"],
        )
        .expect("shard_version_rejected_total vec");
        registry
            .register(Box::new(shard_version_rejected_total.clone()))
            .expect("register shard_version_rejected_total");

        let reconciler_sweeps_total = IntCounter::with_opts(Opts::new(
            "siahub_cas_reconciler_sweeps_total",
            "Reconciler sweeps that handled at least one row (n > 0).",
        ))
        .expect("reconciler_sweeps_total counter");
        registry
            .register(Box::new(reconciler_sweeps_total.clone()))
            .expect("register reconciler_sweeps_total");

        let reconciler_failures_total = IntCounter::with_opts(Opts::new(
            "siahub_cas_reconciler_failures_total",
            "Reconciler iterations that encountered an unexpected error (distinct from per-row pin_attempts bumps).",
        ))
        .expect("reconciler_failures_total counter");
        registry
            .register(Box::new(reconciler_failures_total.clone()))
            .expect("register reconciler_failures_total");

        // Plan Task 2: sia_upload buckets tuned for 64 MiB xorb uploads.
        let sia_upload_duration_seconds = Histogram::with_opts(
            HistogramOpts::new(
                "siahub_cas_sia_upload_duration_seconds",
                "Wall time of sia upload+pin round-trips in seconds.",
            )
            .buckets(vec![0.1, 0.5, 1.0, 5.0, 10.0, 30.0, 60.0]),
        )
        .expect("sia_upload_duration_seconds histogram");
        registry
            .register(Box::new(sia_upload_duration_seconds.clone()))
            .expect("register sia_upload_duration_seconds");

        // Plan Task 2: sia_pin buckets tuned for pin-only retries.
        let sia_pin_duration_seconds = Histogram::with_opts(
            HistogramOpts::new(
                "siahub_cas_sia_pin_duration_seconds",
                "Wall time of sia pin_only round-trips in seconds.",
            )
            .buckets(vec![0.05, 0.1, 0.5, 1.0, 5.0]),
        )
        .expect("sia_pin_duration_seconds histogram");
        registry
            .register(Box::new(sia_pin_duration_seconds.clone()))
            .expect("register sia_pin_duration_seconds");

        Self {
            registry,
            xorb_orphaned_total,
            shard_orphaned_total,
            shard_version_rejected_total,
            reconciler_sweeps_total,
            reconciler_failures_total,
            sia_upload_duration_seconds,
            sia_pin_duration_seconds,
        }
    }

    /// Render the current registry contents in Prometheus text format
    /// (v0.0.4). Cheap: gathers in-process counter handles; no allocation
    /// beyond the output buffer.
    pub fn encode(&self) -> String {
        let mf = self.registry.gather();
        let encoder = TextEncoder::new();
        let mut buf = Vec::with_capacity(4 * 1024);
        // TextEncoder::encode never fails against an in-memory Vec; the
        // `Result` is a holdover from implementations that write to `io::Write`.
        encoder
            .encode(&mf, &mut buf)
            .expect("TextEncoder writes to Vec infallibly");
        String::from_utf8(buf).expect("Prometheus text encoding is UTF-8")
    }
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

/// `Metrics` implements the reconciler's `ReconcilerMetrics` boundary trait
/// this is the cross-crate adapter that keeps `siahub-cas-storage` from
/// importing `siahub-cas-core` (which would form a dependency cycle, since
/// `siahub-cas-core` depends on `siahub-cas-storage` for `SiaAdapter`).
impl ReconcilerMetrics for Metrics {
    fn inc_sweep(&self) {
        self.reconciler_sweeps_total.inc();
    }

    fn inc_failure(&self) {
        self.reconciler_failures_total.inc();
    }

    fn inc_orphaned_xorb(&self) {
        self.xorb_orphaned_total.inc();
    }

    fn inc_orphaned_shard(&self) {
        self.shard_orphaned_total.inc();
    }
}

/// State trait the binary crate's `AppState` implements so handlers can reach
/// the `Metrics` handle set without this crate depending on the concrete
/// state struct — mirrors `AuthStateRef`, `XorbUploadState`, etc.
pub trait MetricsState: Clone + Send + Sync + 'static {
    fn metrics(&self) -> Arc<Metrics>;
}

/// `GET /metrics` handler. Emits Prometheus text format (v0.0.4) with
/// `Content-Type: text/plain; version=0.0.4` — the exact header Prom scrapers
/// look for. No auth: network-level restriction (127.0.0.1 bind) is the
/// trust boundary ( / ).
pub async fn metrics_handler<S: MetricsState>(State(st): State<S>) -> impl IntoResponse {
    let body = st.metrics().encode();
    let mut h = HeaderMap::new();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; version=0.0.4; charset=utf-8"),
    );
    // Scraping budget: stop intermediate caches (Caddy, CDN) from memoizing.
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-store"),
    );
    (h, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metrics_registry_includes_phase_2_series() {
        let m = Metrics::new();
        // Bump some values so the gathered output is non-trivial.
        m.xorb_orphaned_total.inc();
        m.shard_orphaned_total.inc();
        m.reconciler_sweeps_total.inc_by(3);
        m.reconciler_failures_total.inc();
        m.shard_version_rejected_total
            .with_label_values(&["7", "3"])
            .inc();
        m.sia_upload_duration_seconds.observe(0.5);
        m.sia_pin_duration_seconds.observe(0.12);

        let s = m.encode();
        assert!(
            s.contains("siahub_cas_xorb_orphaned_total"),
            "xorb orphaned series present"
        );
        assert!(
            s.contains("siahub_cas_shard_orphaned_total"),
            "shard orphaned series present"
        );
        assert!(
            s.contains("siahub_cas_shard_version_rejected_total"),
            "shard version rejected series present"
        );
        assert!(
            s.contains("siahub_cas_reconciler_sweeps_total"),
            "reconciler sweeps series present"
        );
        assert!(
            s.contains("siahub_cas_reconciler_failures_total"),
            "reconciler failures series present"
        );
        assert!(
            s.contains("siahub_cas_sia_upload_duration_seconds"),
            "sia upload duration histogram present"
        );
        assert!(
            s.contains("siahub_cas_sia_pin_duration_seconds"),
            "sia pin duration histogram present"
        );
        assert!(
            s.contains(r#"header_version="7""#),
            "label rendered verbatim"
        );
    }

    #[test]
    fn encode_returns_utf8_text_format() {
        let m = Metrics::new();
        let s = m.encode();
        // Prometheus text format starts with "# HELP" for the first metric.
        assert!(s.starts_with("# HELP"), "expected Prometheus text header, got: {s}");
    }

    #[test]
    fn reconciler_metrics_impl_delegates_to_counters() {
        let m = Metrics::new();
        ReconcilerMetrics::inc_sweep(&m);
        ReconcilerMetrics::inc_failure(&m);
        ReconcilerMetrics::inc_orphaned_xorb(&m);
        ReconcilerMetrics::inc_orphaned_shard(&m);
        assert_eq!(m.reconciler_sweeps_total.get(), 1);
        assert_eq!(m.reconciler_failures_total.get(), 1);
        assert_eq!(m.xorb_orphaned_total.get(), 1);
        assert_eq!(m.shard_orphaned_total.get(), 1);
    }
}
