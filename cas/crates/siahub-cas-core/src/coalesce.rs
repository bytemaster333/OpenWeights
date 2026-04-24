//! fetch_info coalescing — the ONLY place in the codebase that converts
//! END-EXCLUSIVE byte offsets (as stored in `reconstruction_terms`) to
//! END-INCLUSIVE HTTP Range byte offsets (as served in fetch_info).
//! Invariants (PITFALL — .md gotcha discipline):
//! 1. Input `Term::xorb_byte_start..xorb_byte_end` is END-EXCLUSIVE.
//! 2. Output `ByteRange { start, end_inclusive }` is END-INCLUSIVE.
//! 3. There is exactly ONE `- 1` conversion site in the entire codebase.
//! It lives in `coalesce_terms_by_xorb` and is annotated with a P4
//! comment. Any refactor that duplicates this conversion is a bug.
//! A deliberately-overlapping golden JSON snapshot (see
//! `tests::reconstruction_tests::coalesce_golden`) pins the output shape.
//! Any off-by-one refactor fails the build with an `insta` diff.

use std::collections::BTreeMap;

use serde::Serialize;

use siahub_cas_db::queries::reconstruction::Term;

/// HTTP-Range-style END-INCLUSIVE byte range in a xorb. JSON-serialized as
/// `{"start": u64, "end_inclusive": u64}` for the golden snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ByteRange {
    pub start: u64,
    pub end_inclusive: u64,
}

/// Coalesce per-term byte spans (END-EXCLUSIVE) into minimal covering byte
/// ranges (END-INCLUSIVE), grouped by xorb hash.
/// Result ordering: `BTreeMap` keyed by `[u8; 32]` → deterministic iteration
/// order across runs. This is what makes the golden snapshot stable.
/// See module-level docs for the invariant.
pub fn coalesce_terms_by_xorb(terms: &[Term]) -> BTreeMap<[u8; 32], Vec<ByteRange>> {
    // Step 1 — group by xorb_hash. Spans are (start, end_EXCLUSIVE) as stored
    // in reconstruction_terms (`xorb_byte_start`..`xorb_byte_end`).
    let mut groups: BTreeMap<[u8; 32], Vec<(u64, u64)>> = BTreeMap::new();
    for t in terms {
        // xorb_byte_start..xorb_byte_end — END-EXCLUSIVE byte range in xorb
        // (see migration 0002_xorbs_shards.sql comment + queries::Term
        // doc comment).
        debug_assert!(
            t.xorb_byte_start >= 0 && t.xorb_byte_end >= 0,
            "reconstruction_terms byte offsets must be non-negative (DB CHECK"
        );
        groups
            .entry(t.xorb_hash)
            .or_default()
            .push((t.xorb_byte_start as u64, t.xorb_byte_end as u64));
    }

    // Step 2 — per group: sort, merge overlapping/contiguous spans, convert
    // the final merged END-EXCLUSIVE span to an END-INCLUSIVE HTTP-Range.
    groups
        .into_iter()
        .map(|(xorb, mut spans)| {
            spans.sort_unstable_by_key(|&(s, _)| s);
            let mut merged: Vec<(u64, u64)> = Vec::with_capacity(spans.len());
            for (s, e) in spans {
                if let Some(last) = merged.last_mut() {
                    // `s <= last.1` means overlap or contiguous because END
                    // is EXCLUSIVE. Contiguity (s == last.1) collapses [0,100)
                    // and [100,200) into [0,200).
                    if s <= last.1 {
                        last.1 = last.1.max(e);
                        continue;
                    }
                }
                merged.push((s, e));
            }
            // : END-EXCLUSIVE chunk-byte range → END-INCLUSIVE HTTP Range.
            // This is THE ONE conversion site. Any refactor that duplicates
            // this logic is a bug. See module docs.
            let byte_ranges: Vec<ByteRange> = merged
                .into_iter()
                .map(|(s, e)| {
                    debug_assert!(
                        e > s,
                        "end-exclusive span must be non-empty (e={e} > s={s})"
                    );
                    debug_assert!(e > 0, "end-exclusive must be >0 before converting to inclusive");
                    ByteRange {
                        start: s,
                        end_inclusive: e - 1,
                    }
                })
                .collect();
            (xorb, byte_ranges)
        })
        .collect()
}

#[cfg(test)]
mod inline_tests {
    use super::*;

    fn mk_term(xorb: [u8; 32], bs: i64, be: i64) -> Term {
        Term {
            xorb_hash: xorb,
            xorb_start: 0,
            xorb_end: 1,
            xorb_byte_start: bs,
            xorb_byte_end: be,
            unpacked_start: 0,
            unpacked_end: be - bs,
        }
    }

    #[test]
    fn empty_terms_returns_empty_map() {
        let out = coalesce_terms_by_xorb(&[]);
        assert!(out.is_empty());
    }

    #[test]
    fn touch_boundary_merges_contiguous() {
        // [0,100) and [100,200) are CONTIGUOUS under END-EXCLUSIVE semantics.
        // Merged: [0,200) → END-INCLUSIVE 0..=199.
        let a = [0u8; 32];
        let terms = vec![mk_term(a, 0, 100), mk_term(a, 100, 200)];
        let out = coalesce_terms_by_xorb(&terms);
        assert_eq!(out.len(), 1);
        let ranges = &out[&a];
        assert_eq!(ranges.len(), 1);
        assert_eq!(
            ranges[0],
            ByteRange {
                start: 0,
                end_inclusive: 199,
            }
        );
    }

    #[test]
    fn single_byte_range() {
        // [5,6) is a 1-byte span → END-INCLUSIVE 5..=5.
        let a = [0u8; 32];
        let terms = vec![mk_term(a, 5, 6)];
        let out = coalesce_terms_by_xorb(&terms);
        let ranges = &out[&a];
        assert_eq!(
            ranges[0],
            ByteRange {
                start: 5,
                end_inclusive: 5,
            }
        );
    }

    #[test]
    fn disjoint_ranges_stay_separate() {
        // [0,100) and [200,300) are disjoint → two output ranges.
        let a = [0u8; 32];
        let terms = vec![mk_term(a, 0, 100), mk_term(a, 200, 300)];
        let out = coalesce_terms_by_xorb(&terms);
        let ranges = &out[&a];
        assert_eq!(ranges.len(), 2);
        assert_eq!(
            ranges[0],
            ByteRange {
                start: 0,
                end_inclusive: 99,
            }
        );
        assert_eq!(
            ranges[1],
            ByteRange {
                start: 200,
                end_inclusive: 299,
            }
        );
    }

    #[test]
    fn overlapping_ranges_merge() {
        // [10,50) and [40,80) overlap → merged [10,80) → 10..=79.
        let a = [0u8; 32];
        let terms = vec![mk_term(a, 10, 50), mk_term(a, 40, 80)];
        let out = coalesce_terms_by_xorb(&terms);
        let ranges = &out[&a];
        assert_eq!(ranges.len(), 1);
        assert_eq!(
            ranges[0],
            ByteRange {
                start: 10,
                end_inclusive: 79,
            }
        );
    }

    #[test]
    fn ranges_sort_before_merging() {
        // Input order [40,80), [10,50): sort → merge → [10,80) → 10..=79.
        let a = [0u8; 32];
        let terms = vec![mk_term(a, 40, 80), mk_term(a, 10, 50)];
        let out = coalesce_terms_by_xorb(&terms);
        let ranges = &out[&a];
        assert_eq!(ranges.len(), 1);
        assert_eq!(
            ranges[0],
            ByteRange {
                start: 10,
                end_inclusive: 79,
            }
        );
    }

    #[test]
    fn distinct_xorbs_stay_in_separate_groups() {
        let a = [1u8; 32];
        let b = [2u8; 32];
        let terms = vec![mk_term(a, 0, 100), mk_term(b, 0, 200)];
        let out = coalesce_terms_by_xorb(&terms);
        assert_eq!(out.len(), 2);
        assert_eq!(out[&a].len(), 1);
        assert_eq!(out[&b].len(), 1);
        assert_eq!(
            out[&b][0],
            ByteRange {
                start: 0,
                end_inclusive: 199,
            }
        );
    }
}
