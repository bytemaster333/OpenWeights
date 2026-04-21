//! Integration-style tests for siahub-cas-core handlers.
//!
//! Plan 02-04 Task 5 — P1 canary + P2 short-circuit + body-cap + hash-parse.
//! Tests 3/4/7 (happy-path, dedup, Sia-unavailable-via-handler) defer to
//! Plan 02-10's conformance crate where testcontainers Postgres is wired.

mod metering_tests;
mod reconciler_tests;
mod reconstruction_tests;
mod reconstruction_v2_tests;
mod shards_tests;
mod signed_url_tests;
mod xorbs_tests;
