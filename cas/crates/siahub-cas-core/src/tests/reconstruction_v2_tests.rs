//! Plan 02-07 — V2 reconstruction handler tests (feature-flagged 501).
//!
//! Focus of this file — BOTH code paths behind the V2_RECONSTRUCTION_ENABLED
//! flag (D-18):
//!   1. Flag-off 501 response shape + status (Task 2, Test 1).
//!   2. Flag-on V2 response shape + golden `insta` snapshot (Task 2, Test 4).
//!   3. Pure `build_v2_response` exercising coalesced multi-range descriptors
//!      (shape sanity without routing through axum).
//!
//! Tests that require a live Postgres + Redis (handler-level: 401, 403, 404,
//! 429, 501-after-rate-limit-depletion) defer to Plan 02-10's conformance
//! crate — same split rationale as Plan 02-06 documented in
//! `tests/reconstruction_tests.rs`. That plan's Task 2 (§Tests 1–6) lists six
//! handler-integration tests; 2/3/5/6 need live infra. Tests 1 + 4 are
//! covered HERE at the pure-function seam because the flag gate and V2 shape
//! are the two things Phase 3 flips — pinning them locally catches drift at
//! every `cargo test` without requiring Docker.
//!
//! Deviation: `cargo test -p siahub-cas-core reconstruction::v2` targets this
//! file via `reconstruction_v2_tests`; the plan's path template
//! `reconstruction::v2` matches the test-module namespace convention used
//! here (`tests::reconstruction_v2_tests::*`).

use std::sync::Arc;

use crate::handlers::reconstruction::{ChunkRange, UrlRange};
use crate::handlers::reconstruction_v2::{
    QueryReconstructionResponseV2, XorbFetchInfoV2, build_v2_response, v2_disabled_body,
    v2_flag_off_response,
};
use crate::signed_url::{UrlMinter, UrlSigner};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use base64::Engine as _;
use http_body_util::BodyExt;
use siahub_cas_db::queries::reconstruction::{ReconstructionFile, ReconstructionRow, Term};
use siahub_cas_proto::merklehash::MerkleHash;
use uuid::Uuid;

// -------------------------------------------------------------------------
// Shared fixtures — mirror Plan 02-06's `reconstruction_tests.rs::mk_term`
// so V1 ↔ V2 outputs can be reasoned about side-by-side.
// -------------------------------------------------------------------------

fn mk_term(xorb: [u8; 32], bs: i64, be: i64, cs: i64, ce: i64) -> Term {
    Term {
        xorb_hash: xorb,
        xorb_start: cs,
        xorb_end: ce,
        xorb_byte_start: bs,
        xorb_byte_end: be,
        unpacked_start: 0,
        unpacked_end: be - bs,
    }
}

fn test_signer() -> Arc<dyn UrlMinter> {
    // All-zero key so signatures are deterministic — mirrors V1's test_signer.
    let key_b64 =
        base64::engine::general_purpose::STANDARD.encode([0u8; 32]);
    let base = url::Url::parse("https://cas.test/").unwrap();
    let signer = UrlSigner::new(&key_b64, None, base, 7200).expect("signer");
    Arc::new(signer) as Arc<dyn UrlMinter>
}

// -------------------------------------------------------------------------
// Flag-off branch (Test 1 — V2_RECONSTRUCTION_ENABLED=false → 501).
// -------------------------------------------------------------------------

#[test]
fn flag_off_returns_501_not_implemented() {
    // Direct pure-function call — asserts the exact status code Phase 3 is
    // NOT flipping away from. xet-core's `get_reconstruction_with_version_override`
    // keys on 501 specifically (RESEARCH §2.6).
    let resp = v2_flag_off_response();
    assert_eq!(resp.status(), StatusCode::NOT_IMPLEMENTED);
}

#[tokio::test]
async fn flag_off_body_is_v2_disabled_json() {
    // Extract the body bytes; assert they serialize to the contract JSON shape.
    let resp = v2_flag_off_response();
    let bytes = resp
        .into_response()
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    let parsed: serde_json::Value =
        serde_json::from_slice(&bytes).expect("v2 flag-off body is JSON");
    assert_eq!(parsed, v2_disabled_body());
    assert_eq!(parsed["error"], "V2 reconstruction disabled");
}

#[test]
fn flag_off_body_helper_matches_literal_contract() {
    // Phase 3 + ops tooling may grep for this exact string. Pin it.
    let body = v2_disabled_body();
    assert_eq!(body["error"], "V2 reconstruction disabled");
    assert_eq!(
        serde_json::to_string(&body).unwrap(),
        r#"{"error":"V2 reconstruction disabled"}"#
    );
}

// -------------------------------------------------------------------------
// Flag-on branch (Test 4 — V2_RECONSTRUCTION_ENABLED=true → 200 V2 shape).
// -------------------------------------------------------------------------

#[test]
fn flag_on_build_v2_response_produces_per_xorb_single_url() {
    // 4 terms across 2 xorbs — exactly the Plan 02-06 coalesce scenario so
    // V2 ↔ V1 shape diffs are easy to reason about.
    let xorb_a = [0xaa; 32];
    let xorb_b = [0xbb; 32];
    let row = ReconstructionRow {
        file: ReconstructionFile {
            file_id: [0x11; 32],
            total_size: 12345,
        },
        terms: vec![
            mk_term(xorb_a, 1024, 4096, 0, 8),
            mk_term(xorb_a, 3072, 8192, 6, 16),
            mk_term(xorb_a, 12288, 16384, 24, 32),
            mk_term(xorb_b, 0, 65536, 0, 128),
        ],
    };

    let resp: QueryReconstructionResponseV2 =
        build_v2_response(&row, test_signer().as_ref(), Uuid::nil(), 1_700_000_000);

    // Two xorbs → two fetch_info keys (BTreeMap — deterministic order).
    assert_eq!(resp.fetch_info.len(), 2);

    // xorb_a: terms 1+2 merge to [1024, 8192); term 3 disjoint [12288, 16384).
    // P4 → END-INCLUSIVE: 1024..=8191 and 12288..=16383.
    let a_key = MerkleHash::from(xorb_a).hex();
    let a_entry: &XorbFetchInfoV2 = resp.fetch_info.get(&a_key).expect("xorb_a present");
    assert_eq!(
        a_entry.ranges.len(),
        2,
        "xorb_a has two merged range segments"
    );
    assert_eq!(a_entry.ranges[0].url_range.start, 1024);
    assert_eq!(a_entry.ranges[0].url_range.end_inclusive, 8191);
    assert_eq!(a_entry.ranges[0].chunk_range.start, 0);
    assert_eq!(a_entry.ranges[0].chunk_range.end, 16);
    assert_eq!(a_entry.ranges[1].url_range.start, 12288);
    assert_eq!(a_entry.ranges[1].url_range.end_inclusive, 16383);
    assert_eq!(a_entry.ranges[1].chunk_range.start, 24);
    assert_eq!(a_entry.ranges[1].chunk_range.end, 32);

    // SINGLE URL per xorb — the load-bearing V2 ↔ V1 shape diff.
    assert!(a_entry.url.starts_with("https://cas.test/xorb/"));
    assert!(
        a_entry.url.contains(&a_key),
        "url should embed xorb_a hex path: {}",
        a_entry.url
    );
    // Bounding range stamped into the URL covers ALL segments (1024..=16383).
    // Phase 3 minter upgrade swaps this to per-segment `r=s1-e1,s2-e2,...`
    // (see Phase 3 flip checklist in SUMMARY).
    assert!(
        a_entry.url.contains("r=1024-16383"),
        "url must bound the multi-range descriptor: {}",
        a_entry.url
    );

    // xorb_b — single range 0..=65535.
    let b_key = MerkleHash::from(xorb_b).hex();
    let b_entry: &XorbFetchInfoV2 = resp.fetch_info.get(&b_key).expect("xorb_b present");
    assert_eq!(b_entry.ranges.len(), 1);
    assert_eq!(b_entry.ranges[0].url_range.end_inclusive, 65535);
    assert!(b_entry.url.contains("r=0-65535"));

    // terms[] identical in shape to V1 — one per DB term in insert order.
    assert_eq!(resp.terms.len(), 4);
    // Chunk ranges END-EXCLUSIVE (P4) — passed through from DB without
    // off-by-one.
    assert_eq!(resp.terms[0].range.start, 0);
    assert_eq!(resp.terms[0].range.end, 8);

    // Phase 2 always emits whole-file reconstructions.
    assert_eq!(resp.offset_into_first_range, 0);
}

#[test]
fn flag_on_v2_response_golden_snapshot() {
    // Pin the V2 response JSON so Phase 3's flip is a no-wire-drift operation.
    // Matches `reconstruction_tests::coalesce_golden_snapshot`'s scenario at
    // the V2-shape level.
    let xorb_a = [0xaa; 32];
    let xorb_b = [0xbb; 32];
    let row = ReconstructionRow {
        file: ReconstructionFile {
            file_id: [0x11; 32],
            total_size: 12345,
        },
        terms: vec![
            mk_term(xorb_a, 1024, 4096, 0, 8),
            mk_term(xorb_a, 3072, 8192, 6, 16),
            mk_term(xorb_a, 12288, 16384, 24, 32),
            mk_term(xorb_b, 0, 65536, 0, 128),
        ],
    };

    let resp = build_v2_response(&row, test_signer().as_ref(), Uuid::nil(), 1_700_000_000);

    // Strip the signed URLs before snapshotting — `sig=` is deterministic
    // only at a fixed now_unix, but the snapshot's primary job is pinning
    // the V2 wire SHAPE, not HMAC compatibility (which signed_url_tests
    // pins cross-language).
    let snapshot = SnapshotV2::from_response(&resp);
    insta::assert_json_snapshot!("v2_fetch_info_golden", &snapshot);
}

#[test]
fn flag_on_v2_empty_terms_yields_empty_fetch_info() {
    // Matches V1 behavior — empty-term row is not a server error.
    let row = ReconstructionRow {
        file: ReconstructionFile {
            file_id: [0; 32],
            total_size: 0,
        },
        terms: vec![],
    };
    let resp = build_v2_response(&row, test_signer().as_ref(), Uuid::nil(), 0);
    assert!(resp.terms.is_empty());
    assert!(resp.fetch_info.is_empty());
    assert_eq!(resp.offset_into_first_range, 0);
}

#[test]
fn flag_on_v2_single_range_matches_v1_byte_boundary() {
    // Pin the P4 invariant at the V2 layer: a single END-EXCLUSIVE term
    // [0, 1024) → URL range 0..=1023 (END-INCLUSIVE).
    let xorb = [0xcc; 32];
    let row = ReconstructionRow {
        file: ReconstructionFile {
            file_id: [0x22; 32],
            total_size: 1024,
        },
        terms: vec![mk_term(xorb, 0, 1024, 0, 4)],
    };
    let resp = build_v2_response(&row, test_signer().as_ref(), Uuid::nil(), 1_700_000_000);

    let hex = MerkleHash::from(xorb).hex();
    let entry = resp.fetch_info.get(&hex).expect("xorb present");
    assert_eq!(entry.ranges.len(), 1);
    assert_eq!(entry.ranges[0].url_range.start, 0);
    assert_eq!(entry.ranges[0].url_range.end_inclusive, 1023);
    // URL stamps the same range.
    assert!(entry.url.contains("r=0-1023"), "url={}", entry.url);
    // Canonical signed-URL shape — matches Plan 02-08 contract.
    assert!(entry.url.contains("exp="));
    assert!(entry.url.contains("kid="));
    assert!(entry.url.contains("sig="));
}

#[test]
fn flag_on_v2_json_uses_url_range_end_key() {
    // Wire-shape sanity: V2's `url_range` serializes as `{"start", "end"}`,
    // same as V1 (reuses V1's `UrlRange` type — see handler source).
    let xorb = [0xee; 32];
    let row = ReconstructionRow {
        file: ReconstructionFile {
            file_id: [0x44; 32],
            total_size: 0,
        },
        terms: vec![mk_term(xorb, 0, 42, 0, 1)],
    };
    let resp = build_v2_response(&row, test_signer().as_ref(), Uuid::nil(), 0);
    let s = serde_json::to_string(&resp).unwrap();
    assert!(
        s.contains(r#""url_range":{"start":0,"end":41}"#),
        "v2 response shape drift: {s}"
    );
    // V2 differs from V1 in the fetch_info value shape: one `url` + `ranges[]`
    // per xorb (not one entry per merged range). Pin that too.
    assert!(
        s.contains(r#""ranges":[{"chunk_range":"#),
        "v2 ranges[] shape drift: {s}"
    );
}

// -------------------------------------------------------------------------
// Deferred handler-integration tests (Plan 02-10 conformance crate).
//
// The Plan 02-07 `<tests>` block lists six tests: (1) flag-off 501, (2) unauth
// → 401, (3) wrong scope → 403, (4) flag-on 200 shape, (5) flag-on 404 on
// unknown file_id, (6) rate-limit runs before flag-gate. Tests 1 + 4 are
// pinned above at the pure-function seam. Tests 2, 3, 5, 6 require live
// Postgres + Redis and are owned by Plan 02-10 (same split rationale as
// Plan 02-06; see `reconstruction_tests.rs` module docs). Pinned here as
// inventory so Phase 3 + Plan 02-10 know exactly what to add.
// -------------------------------------------------------------------------

/// Compile-time reference to the handler symbol so a rename of
/// `query_reconstruction_v2` or a signature drift breaks this test file at
/// build time — catches accidental removal of the route's Phase 3 toggle
/// point before a regression lands in CI.
#[allow(dead_code)]
fn _handler_symbol_exists() {
    // Using the fn item as a type probe: the compiler instantiates the path
    // but we never actually call it. Generic `S` is left to be inferred
    // by the const-generic route — using `crate::handlers::reconstruction_v2::query_reconstruction_v2::<TestStub>`
    // would require a test stub that implements ReconstructionState; the
    // `use` below is sufficient as a symbol probe.
    #[allow(unused_imports)]
    use crate::handlers::reconstruction_v2::query_reconstruction_v2;
}

// -------------------------------------------------------------------------
// Snapshot scaffolding — strip the signed URLs so the snapshot pins SHAPE,
// not HMAC output. HMAC byte-identity is covered by `signed_url_tests.rs`
// + the cross-language fixture under `conformance/fixtures/`.
// -------------------------------------------------------------------------

#[derive(serde::Serialize)]
struct SnapshotV2<'a> {
    offset_into_first_range: u64,
    terms: Vec<SnapshotTerm<'a>>,
    fetch_info: std::collections::BTreeMap<String, SnapshotFetch<'a>>,
}

#[derive(serde::Serialize)]
struct SnapshotTerm<'a> {
    hash: &'a str,
    unpacked_length: u64,
    range: ChunkRange,
}

#[derive(serde::Serialize)]
struct SnapshotFetch<'a> {
    url_path: String,
    // strip the signed-URL query string (nondeterministic sig= in different
    // HMAC test setups); keep `r=<start>-<end>` because that's the load-
    // bearing multi-range bounding descriptor Phase 3 verifies.
    url_r_param: Option<String>,
    ranges: Vec<SnapshotSegment<'a>>,
}

#[derive(serde::Serialize)]
struct SnapshotSegment<'a> {
    chunk_range: ChunkRange,
    url_range: UrlRange,
    // phantom so the lifetime 'a is actually used (future-proof against
    // snapshot field additions)
    #[serde(skip)]
    _phantom: std::marker::PhantomData<&'a ()>,
}

impl<'a> SnapshotV2<'a> {
    fn from_response(resp: &'a QueryReconstructionResponseV2) -> Self {
        Self {
            offset_into_first_range: resp.offset_into_first_range,
            terms: resp
                .terms
                .iter()
                .map(|t| SnapshotTerm {
                    hash: t.hash.as_str(),
                    unpacked_length: t.unpacked_length,
                    range: t.range,
                })
                .collect(),
            fetch_info: resp
                .fetch_info
                .iter()
                .map(|(k, v)| {
                    let url = url::Url::parse(&v.url).expect("signer produces valid URL");
                    let url_path = url.path().to_string();
                    let url_r_param = url
                        .query_pairs()
                        .find(|(k, _)| k == "r")
                        .map(|(_, v)| v.into_owned());
                    (
                        k.clone(),
                        SnapshotFetch {
                            url_path,
                            url_r_param,
                            ranges: v
                                .ranges
                                .iter()
                                .map(|s| SnapshotSegment {
                                    chunk_range: s.chunk_range,
                                    url_range: s.url_range,
                                    _phantom: std::marker::PhantomData,
                                })
                                .collect(),
                        },
                    )
                })
                .collect(),
        }
    }
}
