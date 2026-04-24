//!reconstruction::v1 tests.
//! Focus of this file:
//! * golden `insta` snapshot — pins `coalesce_terms_by_xorb`'s JSON
//! output for a deliberately-overlapping multi-xorb scenario. Any
//! off-by-one refactor fails the build with a snapshot diff.
//! * Build-response pure-function tests — exercise `build_response` with a
//! synthetic `ReconstructionRow`, asserting shape + the
//! END-EXCLUSIVE-chunk ↔ END-INCLUSIVE-byte invariants at every site.
//! Full handler tests (single happy path, batch, 404, pin-state exclusion,
//! rate-limit, scope) need a live Postgres + Redis and a testcontainers rig
//!those deferred to (conformance crate) per the plan's
//! explicit escape valve at Task 5 ("if context budget gets tight, move
//! Tests 1 + 2 + 3 to and keep only golden + coalesce here").
//! Deviation logged in the SUMMARY.

use std::sync::Arc;

use crate::coalesce::{self, ByteRange};
use crate::handlers::reconstruction::{QueryReconstructionResponse, build_response};
use crate::signed_url::{UrlMinter, UrlSigner};
use siahub_cas_db::queries::reconstruction::{ReconstructionFile, ReconstructionRow, Term};
use siahub_cas_proto::merklehash::MerkleHash;
use uuid::Uuid;

// -------------------------------------------------------------------------
// golden snapshot — the canonical PITFALL test scenario.
// -------------------------------------------------------------------------

fn mk_term(xorb: [u8; 32], bs: i64, be: i64, cs: i64, ce: i64) -> Term {
    Term {
        xorb_hash: xorb,
        // END-EXCLUSIVE chunk indices.
        xorb_start: cs,
        xorb_end: ce,
        // END-EXCLUSIVE byte offsets — inputs to coalesce.
        xorb_byte_start: bs,
        xorb_byte_end: be,
        // END-EXCLUSIVE unpacked offsets.
        unpacked_start: 0,
        unpacked_end: be - bs,
    }
}

/// Golden snapshot — deliberately-overlapping multi-term scenario per the
/// plan's Task 4 spec. Four terms across two xorbs:
/// * xorb A: [1024, 4096), [3072, 8192), [12288, 16384)
/// - Terms 1+2 overlap and merge to [1024, 8192) END-EXCLUSIVE →
/// 1024..=8191 END-INCLUSIVE.
/// - Term 3 is disjoint → 12288..=16383 END-INCLUSIVE.
/// * xorb B: [0, 65536) — single range → 0..=65535 END-INCLUSIVE.
/// Any off-by-one refactor in `coalesce_terms_by_xorb` fails the insta diff.
#[test]
fn coalesce_golden_snapshot() {
    // Use deterministic hash bytes so the snapshot key order is pinned.
    // BTreeMap<[u8;32], _> → iteration order is lexicographic on bytes; we
    // render the keys as xet-core hex for readability.
    let xorb_a = [0xaa; 32];
    let xorb_b = [0xbb; 32];
    let terms = vec![
        mk_term(xorb_a, 1024, 4096, 0, 8),
        mk_term(xorb_a, 3072, 8192, 6, 16),
        mk_term(xorb_a, 12288, 16384, 24, 32),
        mk_term(xorb_b, 0, 65536, 0, 128),
    ];

    // Coalesce: xorb_a gets [1024..=8191, 12288..=16383], xorb_b gets [0..=65535].
    let out = coalesce::coalesce_terms_by_xorb(&terms);

    // Re-key by hex for snapshot readability. This does NOT coerce any
    // END-EXCLUSIVE → END-INCLUSIVE conversion — it is pure rekeying.
    let by_hex: std::collections::BTreeMap<String, Vec<ByteRange>> = out
        .into_iter()
        .map(|(k, v)| (MerkleHash::from(k).hex(), v))
        .collect();

    insta::assert_json_snapshot!("fetch_info_golden", &by_hex);
}

// -------------------------------------------------------------------------
// build_response — pure-function tests (no DB, no Redis, no Sia).
// -------------------------------------------------------------------------

fn test_signer() -> Arc<dyn UrlMinter> {
    // 32 bytes of base64 = 44 chars; use the all-zero key so the signature is
    // deterministic. This is a TEST-ONLY key; never ship these bytes.
    let key_b64 = base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        [0u8; 32],
    );
    let base = url::Url::parse("https://cas.test/").unwrap();
    let signer = UrlSigner::new(&key_b64, None, base, 7200).expect("signer");
    Arc::new(signer) as Arc<dyn UrlMinter>
}

#[test]
fn build_response_produces_coalesced_fetch_info() {
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
            mk_term(xorb_b, 0, 65536, 0, 128),
        ],
    };
    let resp: QueryReconstructionResponse =
        build_response(&row, test_signer().as_ref(), Uuid::nil(), 1_700_000_000);

    // Two xorbs → two fetch_info keys.
    assert_eq!(resp.fetch_info.len(), 2);
    // xorb_a has TWO overlapping terms merged → 1 entry.
    let a_key = MerkleHash::from(xorb_a).hex();
    let a_entries = resp.fetch_info.get(&a_key).expect("xorb_a present");
    assert_eq!(a_entries.len(), 1);
    // : merged [1024, 8192) END-EXCLUSIVE → 1024..=8191 END-INCLUSIVE.
    assert_eq!(a_entries[0].url_range.start, 1024);
    assert_eq!(a_entries[0].url_range.end_inclusive, 8191);
    // Chunk range union: min(0, 6) .. max(8, 16) = 0..16 END-EXCLUSIVE.
    assert_eq!(a_entries[0].range.start, 0);
    assert_eq!(a_entries[0].range.end, 16);

    // xorb_b: single range, 0..=65535.
    let b_key = MerkleHash::from(xorb_b).hex();
    let b_entries = resp.fetch_info.get(&b_key).expect("xorb_b present");
    assert_eq!(b_entries.len(), 1);
    assert_eq!(b_entries[0].url_range.start, 0);
    assert_eq!(b_entries[0].url_range.end_inclusive, 65535);

    // Terms list order preserved from DB row.
    assert_eq!(resp.terms.len(), 3);
    // First term: chunk range END-EXCLUSIVE (0, 8) — unchanged from input.
    assert_eq!(resp.terms[0].range.start, 0);
    assert_eq!(resp.terms[0].range.end, 8);
}

#[test]
fn build_response_emits_signed_url_per_range() {
    let xorb = [0xcc; 32];
    let row = ReconstructionRow {
        file: ReconstructionFile {
            file_id: [0x22; 32],
            total_size: 1024,
        },
        terms: vec![mk_term(xorb, 0, 1024, 0, 4)],
    };
    let resp = build_response(&row, test_signer().as_ref(), Uuid::nil(), 1_700_000_000);

    let hex = MerkleHash::from(xorb).hex();
    let entries = resp.fetch_info.get(&hex).expect("xorb present");
    assert_eq!(entries.len(), 1);

    // Signed URL has the canonical gateway origin + xorb hex in the path.
    let url = &entries[0].url;
    assert!(url.starts_with("https://cas.test/xorb/"), "url = {url}");
    assert!(url.contains(&hex), "url missing xorb hex: {url}");
    // 's UrlSigner canonical string includes exp+kid+sig+r.
    assert!(url.contains("exp="));
    assert!(url.contains("kid="));
    assert!(url.contains("sig="));
    assert!(url.contains("r=0-1023"), "url missing range: {url}");
}

#[test]
fn build_response_empty_terms_yields_empty_fetch_info() {
    let row = ReconstructionRow {
        file: ReconstructionFile {
            file_id: [0; 32],
            total_size: 0,
        },
        terms: vec![],
    };
    let resp = build_response(&row, test_signer().as_ref(), Uuid::nil(), 0);
    assert!(resp.terms.is_empty());
    assert!(resp.fetch_info.is_empty());
    // offset_into_first_range is always 0 in — gateway handles
    // file-level Range: headers; CAS always serves the whole file's metadata.
    assert_eq!(resp.offset_into_first_range, 0);
}

#[test]
fn build_response_disjoint_ranges_stay_separate() {
    let xorb = [0xdd; 32];
    let row = ReconstructionRow {
        file: ReconstructionFile {
            file_id: [0x33; 32],
            total_size: 0,
        },
        terms: vec![
            mk_term(xorb, 0, 100, 0, 4),
            mk_term(xorb, 200, 300, 8, 12),
        ],
    };
    let resp = build_response(&row, test_signer().as_ref(), Uuid::nil(), 0);
    let entries = resp.fetch_info.get(&MerkleHash::from(xorb).hex()).unwrap();
    assert_eq!(entries.len(), 2);
    // : END-EXCLUSIVE [0, 100) → END-INCLUSIVE 0..=99.
    assert_eq!(entries[0].url_range.end_inclusive, 99);
    assert_eq!(entries[1].url_range.end_inclusive, 299);
}

// -------------------------------------------------------------------------
// Wire-shape sanity — the JSON shape serializes with `end` not
// `end_inclusive` for url_range (matches xet-core).
// -------------------------------------------------------------------------

#[test]
fn response_json_uses_end_not_end_inclusive_key() {
    let xorb = [0xee; 32];
    let row = ReconstructionRow {
        file: ReconstructionFile {
            file_id: [0x44; 32],
            total_size: 0,
        },
        terms: vec![mk_term(xorb, 0, 42, 0, 1)],
    };
    let resp = build_response(&row, test_signer().as_ref(), Uuid::nil(), 0);
    let s = serde_json::to_string(&resp).unwrap();
    assert!(
        s.contains(r#""url_range":{"start":0,"end":41}"#),
        "response shape drift: {s}"
    );
}
