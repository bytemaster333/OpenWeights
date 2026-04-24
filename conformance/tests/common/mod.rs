//! Shared harness used by every `tests/*.rs` integration test.
//! Each test binary opts in via:
//! ```ignore
//! mod common;
//! use common::{spawn, schema_check};
//! ```
//! Must live under `tests/common/` (not `src/`) so the `[dev-dependencies]`
//! block of `conformance/Cargo.toml` — xet_client, testcontainers, reqwest,
//! sqlx, insta — stays off the library's dep graph. This preserves the
//! T-02- invariant: `grep -rn "xet_client" cas/` returns nothing;
//! `conformance`'s library dep tree has no xet_client either.

#![allow(dead_code)] // not every test uses every helper

pub mod schema_check;
pub mod spawn;
