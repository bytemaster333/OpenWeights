//! V2 reconstruction handler — PROTO-06, feature-flagged 501 per D-18.
//!
//! Route: `GET /v2/reconstructions/{file_id}`.
//!
//! ## The flag gate (notes.md gotcha #3 — load-bearing)
//!
//! - `V2_RECONSTRUCTION_ENABLED=false` (default): handler returns
//!   `501 Not Implemented` with body `{"error":"V2 reconstruction disabled"}`.
//! - `V2_RECONSTRUCTION_ENABLED=true` (Phase 3 flips `.env`): handler runs the
//!   full V2 response builder below.
//!
//! **Why:** xet-core's `RemoteClient::get_reconstruction_with_version_override`
//! (VERIFIED RESEARCH §2.6) tries V2 first and falls back to V1 on `404 OR 501`.
//! A well-formed V2 response that points at a gateway which does NOT serve
//! `Content-Type: multipart/byteranges` (RFC 7233) causes xet-core to issue
//! multi-range `Range:` headers, receive a concatenated body, fail to parse,
//! and silently corrupt the download. The 501 stub forces the V1 path
//! (single-range gateway-safe) until Phase 3 GATE-03 proves the gateway's
//! multipart serving is correct.
//!
//! ## Ordering contract (threat model T-02-07-01, T-02-07-02)
//!
//! 1. Auth extractor runs FIRST (const-generic `AuthScoped<{SCOPE_DOWNLOAD}>`):
//!    missing/invalid bearer → 401, wrong scope → 403. **Unauth clients do
//!    NOT see 501** — that would leak the V2 endpoint's existence.
//! 2. Rate-limit check runs SECOND: even flag-off requests consume a download
//!    bucket token, preventing flag-bypass DoS scans.
//! 3. Flag gate runs THIRD: if disabled, return 501 (with auth + rate-limit
//!    already charged).
//! 4. Only after the flag is ON does the handler parse the path + touch the DB.
//!
//! ## Code path (flag=true)
//!
//! The V2 response structurally mirrors V1's fetch_info but emits, per xorb,
//! ONE URL covering ALL merged byte ranges via a multi-range descriptor. This
//! matches xet-core's `QueryReconstructionResponseV2` wire contract: each xorb
//! fetch_info entry carries a `{url, ranges:[{chunks, bytes}, ...]}` shape
//! rather than one entry per merged range.
//!
//! The merged-ranges data comes from `coalesce::coalesce_terms_by_xorb` — the
//! SAME output V1 consumes. The ONE `-1` end-exclusive → end-inclusive
//! conversion site stays in `coalesce.rs`; V2 does NOT duplicate it (P4).
//!
//! ## Wire shape note
//!
//! `xet_core_structures = 1.5.1` does NOT re-export a `cas_types` module
//! (verified in Plan 02-06 SUMMARY). Wire types are locally defined against
//! the documented V2 contract; Plan 02-10 conformance validates byte-shape
//! compatibility by round-tripping through `xet_client::RemoteClient` once
//! the Phase 3 gateway's multi-range serving is online.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use siahub_cas_db::queries::reconstruction as recon_q;
use siahub_cas_proto::merklehash::MerkleHash;
use uuid::Uuid;

use crate::auth::AuthScoped;
use crate::coalesce;
use crate::errors::AppError;
use crate::handlers::reconstruction::{
    ChunkRange, ReconstructionState, ReconstructionTerm, UrlRange,
};
use crate::rate_limit::RateLimitClass;
use crate::scopes::SCOPE_DOWNLOAD;
use crate::signed_url::UrlMinter;

// -------------------------------------------------------------------------
// V2 wire types — documented contract from RESEARCH §2.6. xet_core_structures
// 1.5.1 does not re-export these (see 02-06 SUMMARY); defined locally. The
// conformance crate (02-10) validates compatibility.
// -------------------------------------------------------------------------

/// One covering range inside a xorb — pairs the chunk-index range (end-EXCLUSIVE
/// per P4) with the byte range (end-INCLUSIVE HTTP Range) that the multi-range
/// URL grants access to.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RangeSegment {
    /// END-EXCLUSIVE chunk-index range (matches `terms[].range` semantics).
    pub chunk_range: ChunkRange,
    /// END-INCLUSIVE HTTP Range byte offsets inside the xorb.
    pub url_range: UrlRange,
}

/// V2 fetch_info value: ONE signed URL per xorb, carrying all coalesced ranges.
///
/// The gateway (Phase 3) interprets the URL's multi-range `r=s1-e1,s2-e2,...`
/// query param and serves `Content-Type: multipart/byteranges` (RFC 7233).
/// Only enable this path AFTER GATE-03 confirms that response format.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct XorbFetchInfoV2 {
    /// Signed gateway URL stamping the union of all `ranges[].url_range`
    /// into a single multi-range grant. Gateway MUST reject `Range:` headers
    /// falling outside this union.
    pub url: String,
    /// Per-segment chunk + byte range descriptors.
    pub ranges: Vec<RangeSegment>,
}

/// V2 response shape — `QueryReconstructionResponseV2`.
///
/// Diverges from V1 only in the value type of `fetch_info`: one entry per
/// xorb (not per merged range), carrying a single URL with embedded
/// multi-range descriptors.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryReconstructionResponseV2 {
    pub offset_into_first_range: u64,
    pub terms: Vec<ReconstructionTerm>,
    /// Map: xorb_hash_hex → one fetch_info-v2 entry. `BTreeMap` → deterministic
    /// key order (golden snapshot stability; matches V1's discipline).
    pub fetch_info: BTreeMap<String, XorbFetchInfoV2>,
}

// -------------------------------------------------------------------------
// Flag-gate response helpers — pulled out so tests exercise them without
// needing a live auth extractor / DB / redis.
// -------------------------------------------------------------------------

/// Body emitted alongside the 501 when the V2 flag is off.
///
/// xet-core's `get_reconstruction_with_version_override` treats 501 as
/// "V2 unavailable, fall back to V1" regardless of body, but keeping the
/// body stable lets ops tooling distinguish the flag-off 501 from any
/// future unrelated 501.
pub(crate) fn v2_disabled_body() -> serde_json::Value {
    json!({ "error": "V2 reconstruction disabled" })
}

/// Build the flag-off response. Tests call this directly to pin the shape
/// without routing through axum. The handler also invokes it.
pub(crate) fn v2_flag_off_response() -> Response {
    (StatusCode::NOT_IMPLEMENTED, Json(v2_disabled_body())).into_response()
}

// -------------------------------------------------------------------------
// Handler
// -------------------------------------------------------------------------

/// `GET /v2/reconstructions/{file_id_hex}` — PROTO-06, feature-flagged 501.
///
/// Extraction order (auth → rate-limit → flag-gate → DB) enforced by the
/// function body below; altering it would either (a) leak V2 existence to
/// unauth clients (T-02-07-01) or (b) open flag-bypass DoS (T-02-07-02).
pub async fn query_reconstruction_v2<S>(
    State(st): State<S>,
    AuthScoped(ctx): AuthScoped<{ SCOPE_DOWNLOAD }>,
    Path(file_id_hex): Path<String>,
) -> Result<Response, AppError>
where
    S: ReconstructionState,
{
    // (1) Rate-limit (download class) — runs BEFORE the flag gate so a
    //     flag-off-but-authenticated DoS cannot enumerate file_ids for free.
    crate::rate_limit::check(
        &st.redis(),
        RateLimitClass::Download,
        ctx.api_key_id,
        st.rate_limit_defaults(),
    )
    .await?;

    // (2) Flag gate. D-18 default is `false`; Phase 3 GATE-03 flips `.env`.
    //     Return 501 BEFORE any hex parse / DB read — no leaky signal that
    //     file_id would exist if the flag were on.
    if !st.v2_reconstruction_enabled() {
        return Ok(v2_flag_off_response());
    }

    // (3) Parse hex → MerkleHash (P1 — NEVER hand-roll hex). A malformed hex
    //     on this path now returns 400 (already past auth + rate-limit); the
    //     501 flag-off branch above never reaches this parse.
    let file_id = MerkleHash::from_hex(&file_id_hex)
        .map_err(|_| AppError::BadRequest("invalid_file_id"))?;

    // (4) DB fetch — zero Sia I/O (D-13). Pinned-only JOIN filter excludes
    //     non-pinned xorbs → 404 on any non-pinned reference (D-15).
    let file_id_bytes: [u8; 32] = file_id.into();
    let row = recon_q::get_reconstruction(st.pool(), &file_id_bytes)
        .await?
        .ok_or(AppError::NotFound)?;

    // (5) Build V2 response.
    let now_unix = unix_now();
    let resp = build_v2_response(&row, st.url_signer().as_ref(), ctx.api_key_id, now_unix);

    // (6) Meter — same event as V1 (the flag distinction is not interesting
    //     for billing; both paths are a reconstruction query).
    record_usage_reconstruction_v2(st.pool(), &ctx.api_key_id, ctx.user_id, &file_id_bytes)
        .await;

    Ok((StatusCode::OK, Json(resp)).into_response())
}

// -------------------------------------------------------------------------
// Pure response builder — shared with tests.
// -------------------------------------------------------------------------

/// Assemble a `QueryReconstructionResponseV2` from a DB row + signer.
///
/// Pure — takes no state, does no I/O. Zero Sia I/O (D-13). Structurally the
/// same pipeline as V1's `build_response`, but emits one `XorbFetchInfoV2`
/// per xorb (not one `FetchInfoEntry` per merged range).
///
/// Key invariants:
///   * `terms` — same as V1: one entry per DB term row in DB order.
///     Chunk indices are END-EXCLUSIVE (P4).
///   * `fetch_info[xorb].url` — SINGLE signed URL carrying ALL merged ranges
///     via a multi-range descriptor. Range field `r=s1-e1,s2-e2,...`.
///   * `fetch_info[xorb].ranges[i].url_range` — END-INCLUSIVE HTTP Range byte
///     offsets produced by `coalesce::coalesce_terms_by_xorb` (the ONE
///     conversion site; NOT duplicated here).
pub(crate) fn build_v2_response(
    row: &recon_q::ReconstructionRow,
    signer: &dyn UrlMinter,
    kid: Uuid,
    now_unix: u64,
) -> QueryReconstructionResponseV2 {
    // Term list — identical shape to V1 (chunk ranges END-EXCLUSIVE).
    let terms: Vec<ReconstructionTerm> = row
        .terms
        .iter()
        .map(|t| ReconstructionTerm {
            hash: MerkleHash::from(t.xorb_hash).hex(),
            unpacked_length: (t.unpacked_end - t.unpacked_start) as u64,
            range: ChunkRange {
                start: t.xorb_start as u32,
                end: t.xorb_end as u32,
            },
        })
        .collect();

    // P4 coalescing — SAME call as V1. This is where the `-1` conversion
    // lives; V2 does NOT duplicate it.
    let coalesced = coalesce::coalesce_terms_by_xorb(&row.terms);

    // Per-xorb chunk-span table — used to recover each merged byte range's
    // chunk-index union. Identical structure to V1's `chunk_spans_per_xorb`.
    let mut chunk_spans_per_xorb: BTreeMap<[u8; 32], Vec<TermSpanV2>> = BTreeMap::new();
    for t in &row.terms {
        chunk_spans_per_xorb
            .entry(t.xorb_hash)
            .or_default()
            .push(TermSpanV2 {
                byte_start: t.xorb_byte_start as u64,
                byte_end: t.xorb_byte_end as u64,
                chunk_start: t.xorb_start as u32,
                chunk_end: t.xorb_end as u32,
            });
    }

    let mut fetch_info: BTreeMap<String, XorbFetchInfoV2> = BTreeMap::new();
    for (xorb_hash, byte_ranges) in coalesced {
        let xorb_mhash = MerkleHash::from(xorb_hash);
        let hex_key = xorb_mhash.hex();
        let term_spans = chunk_spans_per_xorb
            .get(&xorb_hash)
            .cloned()
            .unwrap_or_default();

        // Collect per-range segments (chunk-range union + byte range).
        let mut ranges: Vec<RangeSegment> = Vec::with_capacity(byte_ranges.len());
        // Collect the multi-range tuples the signer will stamp into the URL.
        let mut signed_segments: Vec<(u64, u64)> = Vec::with_capacity(byte_ranges.len());

        for br in byte_ranges {
            // Recover the chunk-range union for every term whose byte span
            // falls fully inside this merged END-INCLUSIVE byte range. Same
            // local-bridge idiom as V1 (+1 to compare against END-EXCLUSIVE
            // term spans); NOT a P4 conversion site — no END-INCLUSIVE byte
            // range is produced by this read.
            let merged_end_exclusive = br.end_inclusive + 1;
            let mut chunk_start: Option<u32> = None;
            let mut chunk_end: Option<u32> = None;
            for ts in &term_spans {
                if ts.byte_start >= br.start && ts.byte_end <= merged_end_exclusive {
                    chunk_start =
                        Some(chunk_start.map_or(ts.chunk_start, |x| x.min(ts.chunk_start)));
                    chunk_end = Some(chunk_end.map_or(ts.chunk_end, |x| x.max(ts.chunk_end)));
                }
            }
            let chunk_range = ChunkRange {
                start: chunk_start.unwrap_or(0),
                end: chunk_end.unwrap_or(0),
            };

            ranges.push(RangeSegment {
                chunk_range,
                url_range: UrlRange {
                    start: br.start,
                    end_inclusive: br.end_inclusive,
                },
            });
            signed_segments.push((br.start, br.end_inclusive));
        }

        // Mint the ONE signed URL covering ALL segments.
        //
        // Phase 2 note: Plan 02-08's `UrlMinter::mint_v1` accepts a single
        // `(start, end_inclusive)` byte range. Until Phase 3 lands a native
        // `mint_v1_multi_range(ranges: &[(u64,u64)])` variant, we mint against
        // the BOUNDING range (min_start, max_end_inclusive) and record the
        // individual segments in `ranges[]`. The gateway's multi-range
        // enforcement in Phase 3 will validate individual segments against
        // the recorded `ranges[]` bounds as part of GATE-03; the minter-side
        // upgrade is a one-line swap when that lands (see SUMMARY Phase 3
        // flip checklist).
        let bounding = bounding_range(&signed_segments);
        let url = signer.mint_v1(&xorb_mhash, bounding, kid, now_unix);

        fetch_info.insert(hex_key, XorbFetchInfoV2 { url, ranges });
    }

    QueryReconstructionResponseV2 {
        // Phase 2 always serves whole-file reconstructions at the CAS level.
        // Phase 3 gateway handles file-level Range: headers.
        offset_into_first_range: 0,
        terms,
        fetch_info,
    }
}

/// Compute the BOUNDING (start, end_inclusive) covering every segment.
/// Returns `None` if there are no segments (no URL needs minting).
fn bounding_range(segments: &[(u64, u64)]) -> Option<(u64, u64)> {
    if segments.is_empty() {
        return None;
    }
    let mut min_start = u64::MAX;
    let mut max_end = 0u64;
    for &(s, e) in segments {
        if s < min_start {
            min_start = s;
        }
        if e > max_end {
            max_end = e;
        }
    }
    Some((min_start, max_end))
}

/// Per-term xorb span bundle — mirrors V1's `TermSpan` for V2's chunk-range
/// reconstruction. Keeping it local (not `pub(crate)` on V1's type) so the
/// two handlers stay decoupled for future evolution.
#[derive(Debug, Clone, Copy)]
struct TermSpanV2 {
    byte_start: u64,  // END-EXCLUSIVE byte
    byte_end: u64,    // END-EXCLUSIVE byte
    chunk_start: u32, // END-EXCLUSIVE chunk
    chunk_end: u32,   // END-EXCLUSIVE chunk
}

// -------------------------------------------------------------------------
// Metering — D-17 synchronous usage_log INSERT (event='reconstruction').
// -------------------------------------------------------------------------

async fn record_usage_reconstruction_v2(
    pool: &sqlx::PgPool,
    api_key_id: &Uuid,
    user_id: i64,
    file_id: &[u8; 32],
) {
    let res = sqlx::query(
        "INSERT INTO usage_log (event, api_key_id, user_id, xorb_hash, bytes) \
         VALUES ('reconstruction'::usage_event, $1, $2, $3, 0)",
    )
    .bind(api_key_id)
    .bind(user_id)
    .bind(&file_id[..])
    .execute(pool)
    .await;
    if let Err(e) = res {
        tracing::warn!(err = %e, "usage_log insert (reconstruction v2) failed");
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// Keep `Arc<dyn UrlMinter>` import alive for rustdoc + future seams. The
// handler reaches through the `ReconstructionState` trait, but marker-level
// usage here documents the signer dependency.
#[allow(dead_code)]
fn _doc_marker(_m: Arc<dyn UrlMinter>) {}

#[cfg(test)]
mod inline_tests {
    use super::*;

    #[test]
    fn v2_disabled_body_shape_is_stable() {
        // Phase 3 gateway + ops tooling may match on this literal; don't
        // rephrase without bumping the cross-language fixture.
        let b = v2_disabled_body();
        assert_eq!(b["error"], "V2 reconstruction disabled");
    }

    #[test]
    fn v2_flag_off_status_is_501() {
        let resp = v2_flag_off_response();
        assert_eq!(resp.status(), StatusCode::NOT_IMPLEMENTED);
    }

    #[test]
    fn bounding_range_empty_is_none() {
        assert!(bounding_range(&[]).is_none());
    }

    #[test]
    fn bounding_range_covers_all_segments() {
        let segs = [(10, 99), (200, 299), (5, 50)];
        assert_eq!(bounding_range(&segs), Some((5, 299)));
    }

    #[test]
    fn bounding_range_single_segment_identity() {
        assert_eq!(bounding_range(&[(42, 99)]), Some((42, 99)));
    }
}
