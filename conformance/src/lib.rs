//! `siahub-conformance` — the Phase 2 Xet-protocol end-to-end conformance
//! harness. Drives the CAS via `xet_client = "=1.5.1"` (dev-dep only; see
//! `Cargo.toml` for the pin rationale + T-02-10-06 guard).
//!
//! This library crate intentionally stays minimal — it holds ONLY the
//! pure-CPU fixture loaders and process-wide helpers. All test harness code
//! that touches `xet_client`, `testcontainers`, `reqwest`, or `sqlx` lives
//! under `tests/common/` so the dev-only deps never leak into the library
//! dep graph (T-02-10-06).
//!
//! Integration tests (`tests/*.rs`) import:
//!   - `siahub_conformance::fixtures::*` from here (lib crate);
//!   - `crate::common::{spawn, schema_check}` from the per-test binary
//!     (via `mod common;` in each `tests/*.rs`).
//!
//! Tests skip with `eprintln!` — never fail — when Docker / fixtures are
//! absent.

pub mod fixtures;

/// Initialize a JSON-formatted tracing subscriber exactly once per process.
/// Safe to call from every `#[tokio::test]`.
pub fn init_tracing_once() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = tracing_subscriber::fmt()
            .json()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
            )
            .try_init();
    });
}

/// The pinned reference xorb hash from `xet-team/xet-spec-reference-files`.
/// Re-exported here as a `const &str` so tests don't need to import
/// `siahub_cas_proto::merklehash` just to reference it.
pub const REFERENCE_XORB_HASH_HEX: &str =
    "eea25d6ee393ccae385820daed127b96ef0ea034dfb7cf6da3a950ce334b7632";

/// The pinned fixture revision. Any bump requires a planning step — enforce
/// with `assert_eq!` in tests that load remote data.
pub const FIXTURE_REVISION: &str = "18bf9173fb2ca80ab3a6fdff81119ff61be7e7dd";
