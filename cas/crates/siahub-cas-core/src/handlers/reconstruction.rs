//! V1 reconstruction handlers — PROTO-05 (single) + PROTO-07 (batch).
//!
//! Routes:
//!   * `GET /v1/reconstructions/{file_id}`       → `query_reconstruction_v1`
//!   * `GET /v1/reconstructions?file_id=A&...`   → `query_reconstructions_batch_v1`
//!   * `GET /reconstructions?file_id=A&...`      → `query_reconstructions_batch_v1`
//!     (dual-path — xet-core's `RemoteClient::batch_get_reconstruction`
//!     constructs `"{endpoint}/reconstructions?"` without `/v1`.)
//!
//! Pitfall ownership (CONTEXT §5):
//!   * **P4** — `fetch_info` coalescing into minimal covering ranges per xorb.
//!     Delegated to `crate::coalesce::coalesce_terms_by_xorb`, the ONE
//!     conversion site.
//!   * **D-13 invariant** — zero Sia I/O on this path. Handler only consults
//!     Postgres (reconstruction_{files,terms} JOIN pinned xorbs).
//!   * **D-15** — reconstruction JOIN filters to `pin_state='pinned'` so
//!     non-pinned xorbs surface as 404 rather than confusing partial
//!     responses.
//!   * **D-17** — `usage_log` INSERT with `event='reconstruction'` runs
//!     synchronously in the handler path (Phase 2 metering).
//!
//! Wire shape (xet-core `QueryReconstructionResponse` V1, sourced from
//! FEATURES.md line 36 + RESEARCH §2.5 + PATTERNS.md §reconstruction.rs).
//!
//! `xet_core_structures = 1.5.1` does NOT re-export `cas_types`. The types
//! below are locally defined and matched against the HF xet protocol docs;
//! Plan 02-10's conformance crate will validate byte-shape compatibility by
//! round-tripping fixtures through `xet_client::RemoteClient::query_reconstruction`.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, Query, State},
};
use serde::{Deserialize, Serialize};

use siahub_cas_db::queries::reconstruction as recon_q;
use siahub_cas_proto::merklehash::MerkleHash;
use uuid::Uuid;

use crate::auth::{AuthScoped, AuthStateRef};
use crate::coalesce;
use crate::errors::AppError;
use crate::rate_limit::{RateLimitClass, RateLimitDefaults};
use crate::scopes::SCOPE_DOWNLOAD;
use crate::signed_url::UrlMinter;

// -------------------------------------------------------------------------
// Wire types — JSON shape xet-core's `RemoteClient::query_reconstruction`
// deserializes into. Keep snake_case; never rename without bumping conformance.
// -------------------------------------------------------------------------

/// END-EXCLUSIVE chunk-index range. Appears in `terms[].range` and
/// `fetch_info[xorb][i].range`.
///
/// Note: both `start` AND `end` are chunk indices (not bytes). xet-core walks
/// fetch_info by matching `fi.range.start == term.range.end` (PITFALLS P4
/// line 94); keeping this END-EXCLUSIVE is a correctness requirement.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChunkRange {
    pub start: u32,
    pub end: u32,
}

/// END-INCLUSIVE HTTP-Range byte range inside a xorb (HTTP Range semantics).
/// Produced by `coalesce_terms_by_xorb` → `ByteRange`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct UrlRange {
    pub start: u64,
    /// END-INCLUSIVE byte offset (HTTP Range). Serialized under the key
    /// `"end"` to match xet-core's wire type.
    #[serde(rename = "end")]
    pub end_inclusive: u64,
}

/// One entry in the `terms[]` array. `unpacked_length` = bytes in the
/// reconstructed output contributed by this term.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReconstructionTerm {
    pub hash: String, // xorb hash hex (xet-core MerkleHash codec)
    pub unpacked_length: u64,
    pub range: ChunkRange, // END-EXCLUSIVE chunk indices in the xorb
}

/// One entry in a `fetch_info` map value list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchInfoEntry {
    pub range: ChunkRange, // END-EXCLUSIVE chunk indices — matches term.range
    pub url: String,
    pub url_range: UrlRange, // END-INCLUSIVE bytes — HTTP Range
}

/// V1 response shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryReconstructionResponse {
    pub offset_into_first_range: u64,
    pub terms: Vec<ReconstructionTerm>,
    /// Map: xorb_hash_hex → list of fetch_info entries. Keys are sorted for
    /// deterministic output (golden snapshot stability).
    pub fetch_info: BTreeMap<String, Vec<FetchInfoEntry>>,
}

/// Batch response: map of file_id_hex → single response. Missing file_ids
/// are omitted per xet-core's `batch_get_reconstruction` expectation
/// (validated at Plan 02-10).
pub type BatchQueryReconstructionResponse = BTreeMap<String, QueryReconstructionResponse>;

/// Query params for batch endpoint: repeated `file_id=<hex>` entries.
#[derive(Debug, Deserialize)]
pub struct BatchQueryParams {
    #[serde(default)]
    pub file_id: Vec<String>,
}

// -------------------------------------------------------------------------
// State trait — handler reaches redis/url-signer via the concrete
// AppState without the core crate depending on the binary (cycle).
// -------------------------------------------------------------------------

/// Minimum state surface the reconstruction handlers consume.
///
/// Mirrors the `XorbUploadState` pattern from Plan 02-04. Binary's AppState
/// implements this; handler stays decoupled from the concrete type.
///
/// `url_signer` is a `dyn UrlMinter` trait object so:
///   - Plan 02-08 provides the concrete `UrlSigner` (HMAC-SHA256) now.
///   - Tests can inject a stub that emits `"TBD"` URLs without HMAC wiring.
pub trait ReconstructionState: AuthStateRef {
    fn redis(&self) -> Arc<fred::clients::Client>;
    fn rate_limit_defaults(&self) -> RateLimitDefaults;
    fn url_signer(&self) -> Arc<dyn UrlMinter>;

    /// D-18 flag gate for V2 reconstruction. Phase 2 default is `false`;
    /// Phase 3 GATE-03 flips this to `true` via `.env` only (no code change).
    /// notes.md gotcha #3 — enabling V2 before the gateway serves
    /// `multipart/byteranges` silently corrupts xet-core downloads. Kept on
    /// the trait (not Config directly) so handler tests can pin both branches
    /// without booting a full binary.
    fn v2_reconstruction_enabled(&self) -> bool;
}

// -------------------------------------------------------------------------
// Handlers
// -------------------------------------------------------------------------

/// `GET /v1/reconstructions/{file_id_hex}` — PROTO-05.
pub async fn query_reconstruction_v1<S>(
    State(st): State<S>,
    AuthScoped(ctx): AuthScoped<{ SCOPE_DOWNLOAD }>,
    Path(file_id_hex): Path<String>,
) -> Result<Json<QueryReconstructionResponse>, AppError>
where
    S: ReconstructionState,
{
    // (1) Parse hex → MerkleHash — P1 (NEVER hand-roll hex).
    let file_id = MerkleHash::from_hex(&file_id_hex)
        .map_err(|_| AppError::BadRequest("invalid_file_id"))?;

    // (2) Rate-limit (download class).
    crate::rate_limit::check(
        &st.redis(),
        RateLimitClass::Download,
        ctx.api_key_id,
        st.rate_limit_defaults(),
    )
    .await?;

    // (3) DB fetch — zero Sia I/O (D-13). Pinned-only JOIN filter excludes
    //     non-pinned xorbs; if every term's xorb is non-pinned, this returns
    //     None → 404 (D-15).
    let file_id_bytes: [u8; 32] = file_id.into();
    let row = recon_q::get_reconstruction(st.pool(), &file_id_bytes)
        .await?
        .ok_or(AppError::NotFound)?;

    // (4) Build response body.
    let now_unix = unix_now();
    let resp = build_response(&row, st.url_signer().as_ref(), ctx.api_key_id, now_unix);

    // (5) Meter (best-effort; logged on failure).
    crate::metering::log_on_err(
        "usage_log insert (reconstruction) failed",
        crate::metering::record_reconstruction(st.pool(), &ctx, &file_id_bytes).await,
    );

    Ok(Json(resp))
}

/// `GET /v1/reconstructions?file_id=A&file_id=B` AND
/// `GET /reconstructions?file_id=A&file_id=B` — PROTO-07.
///
/// Dual-path: xet-core's `batch_get_reconstruction` calls
/// `{endpoint}/reconstructions?` (no `/v1`) per RESEARCH §2.5, while the
/// OpenAPI spec lists `/v1/reconstructions?`. Register both.
pub async fn query_reconstructions_batch_v1<S>(
    State(st): State<S>,
    AuthScoped(ctx): AuthScoped<{ SCOPE_DOWNLOAD }>,
    Query(params): Query<BatchQueryParams>,
) -> Result<Json<BatchQueryReconstructionResponse>, AppError>
where
    S: ReconstructionState,
{
    // (1) Parse every file_id hex — ANY parse failure → 400.
    let mut file_ids: Vec<[u8; 32]> = Vec::with_capacity(params.file_id.len());
    let mut hex_for_id: BTreeMap<[u8; 32], String> = BTreeMap::new();
    for fid_hex in &params.file_id {
        let h = MerkleHash::from_hex(fid_hex)
            .map_err(|_| AppError::BadRequest("invalid_file_id"))?;
        let bytes: [u8; 32] = h.into();
        file_ids.push(bytes);
        hex_for_id.insert(bytes, h.hex());
    }

    // (2) Rate-limit (once per batch request, not per file_id — aligns with
    //     xet-core's single HTTP call → single bucket decrement).
    crate::rate_limit::check(
        &st.redis(),
        RateLimitClass::Download,
        ctx.api_key_id,
        st.rate_limit_defaults(),
    )
    .await?;

    // (3) Single batched DB fetch.
    let rows = recon_q::get_reconstructions_batch(st.pool(), &file_ids).await?;

    // (4) Build batch response (BTreeMap: deterministic; missing file_ids
    //     simply absent).
    let signer = st.url_signer();
    let kid = ctx.api_key_id;
    let now_unix = unix_now();
    let mut out: BatchQueryReconstructionResponse = BTreeMap::new();
    for (fid_bytes, row) in rows {
        let hex = hex_for_id
            .get(&fid_bytes)
            .cloned()
            .unwrap_or_else(|| MerkleHash::from(fid_bytes).hex());
        let resp = build_response(&row, signer.as_ref(), kid, now_unix);
        out.insert(hex, resp);

        // Meter each file as a distinct reconstruction event (per-file
        // counter is what CONSOLE-03..08 expects — one row per file_id).
        crate::metering::log_on_err(
            "usage_log insert (reconstruction, batch) failed",
            crate::metering::record_reconstruction(st.pool(), &ctx, &fid_bytes).await,
        );
    }

    Ok(Json(out))
}

// -------------------------------------------------------------------------
// Shared response builder — pure function; used by both single + batch.
// -------------------------------------------------------------------------

/// Per-term xorb span bundle — both byte and chunk ranges are END-EXCLUSIVE
/// (P4). Used internally by `build_response` to recover chunk-range unions
/// per merged byte range without fanning a 4-tuple through the function.
#[derive(Debug, Clone, Copy)]
struct TermSpan {
    /// END-EXCLUSIVE byte offset inside the xorb.
    byte_start: u64,
    /// END-EXCLUSIVE byte offset inside the xorb.
    byte_end: u64,
    /// END-EXCLUSIVE chunk index inside the xorb.
    chunk_start: u32,
    /// END-EXCLUSIVE chunk index inside the xorb.
    chunk_end: u32,
}

/// Assemble a `QueryReconstructionResponse` from a DB row + signer.
///
/// Pure — takes no state, does no I/O. Zero Sia I/O (D-13). The ONLY work
/// here beyond coalescing is calling `signer.mint_v1` for each merged range.
pub(crate) fn build_response(
    row: &recon_q::ReconstructionRow,
    signer: &dyn UrlMinter,
    kid: Uuid,
    now_unix: u64,
) -> QueryReconstructionResponse {
    // Terms list: one per DB term row, in DB order. chunk indices are
    // END-EXCLUSIVE (P4 — same as in reconstruction_terms + xet-core wire).
    let terms: Vec<ReconstructionTerm> = row
        .terms
        .iter()
        .map(|t| ReconstructionTerm {
            hash: MerkleHash::from(t.xorb_hash).hex(),
            unpacked_length: (t.unpacked_end - t.unpacked_start) as u64,
            // END-EXCLUSIVE chunk indices — passed through without any off-by-one.
            range: ChunkRange {
                start: t.xorb_start as u32,
                end: t.xorb_end as u32,
            },
        })
        .collect();

    // P4 coalescing: merge overlapping/contiguous byte spans per xorb.
    // The coalesce module is the ONLY end-exclusive → end-inclusive
    // conversion site; here we just consume its output.
    let coalesced = coalesce::coalesce_terms_by_xorb(&row.terms);

    // Collect per-xorb term spans — needed to reconstruct the chunk-range
    // union for each merged byte range.
    //   * byte_start..byte_end   — END-EXCLUSIVE byte span in xorb.
    //   * chunk_start..chunk_end — END-EXCLUSIVE chunk-index span.
    let mut chunk_spans_per_xorb: BTreeMap<[u8; 32], Vec<TermSpan>> = BTreeMap::new();
    for t in &row.terms {
        chunk_spans_per_xorb
            .entry(t.xorb_hash)
            .or_default()
            .push(TermSpan {
                byte_start: t.xorb_byte_start as u64,
                byte_end: t.xorb_byte_end as u64, // END-EXCLUSIVE byte
                chunk_start: t.xorb_start as u32,
                chunk_end: t.xorb_end as u32, // END-EXCLUSIVE chunk
            });
    }

    // fetch_info: keyed by xorb_hash_hex, deterministic (BTreeMap).
    let mut fetch_info: BTreeMap<String, Vec<FetchInfoEntry>> = BTreeMap::new();
    for (xorb_hash, byte_ranges) in coalesced {
        let xorb_mhash = MerkleHash::from(xorb_hash);
        let hex_key = xorb_mhash.hex();
        let term_spans = chunk_spans_per_xorb
            .get(&xorb_hash)
            .cloned()
            .unwrap_or_default();

        let mut entries: Vec<FetchInfoEntry> = Vec::with_capacity(byte_ranges.len());
        for br in byte_ranges {
            // Recover the chunk-range union for every term whose byte span
            // falls fully inside this merged (end-INCLUSIVE) byte range.
            //
            // Byte spans in `term_spans` are END-EXCLUSIVE; merged `br` is
            // END-INCLUSIVE. We bridge by adding 1 to `br.end_inclusive` to
            // recover an END-EXCLUSIVE comparison bound. This is a LOCAL
            // read, NOT a conversion back to end-exclusive for a byte-range
            // output, so it is NOT a P4 conversion site.
            let merged_end_exclusive = br.end_inclusive + 1;
            let mut chunk_start: Option<u32> = None;
            let mut chunk_end: Option<u32> = None;
            for ts in &term_spans {
                if ts.byte_start >= br.start && ts.byte_end <= merged_end_exclusive {
                    chunk_start = Some(chunk_start.map_or(ts.chunk_start, |x| x.min(ts.chunk_start)));
                    chunk_end = Some(chunk_end.map_or(ts.chunk_end, |x| x.max(ts.chunk_end)));
                }
            }
            let chunk_range = ChunkRange {
                start: chunk_start.unwrap_or(0),
                end: chunk_end.unwrap_or(0),
            };

            // Mint signed URL for this (xorb_hash, byte_range). The signer
            // expects end-INCLUSIVE range tuple per its canonical-string
            // contract — same semantics as `br.end_inclusive`.
            let url = signer.mint_v1(
                &xorb_mhash,
                Some((br.start, br.end_inclusive)),
                kid,
                now_unix,
            );

            entries.push(FetchInfoEntry {
                range: chunk_range,
                url,
                url_range: UrlRange {
                    start: br.start,
                    end_inclusive: br.end_inclusive,
                },
            });
        }
        fetch_info.insert(hex_key, entries);
    }

    QueryReconstructionResponse {
        // Phase 2 always serves whole-file reconstructions. HTTP Range
        // header handling on the file level is Phase 3 gateway concern.
        offset_into_first_range: 0,
        terms,
        fetch_info,
    }
}

// -------------------------------------------------------------------------
// Metering — routed through `crate::metering` (Plan 02-09 / D-17).
// Event='reconstruction' writes file_id into the usage_log.file_id column;
// no local insert helper remains here — the facade is the only call-site.
// -------------------------------------------------------------------------

/// Current Unix time in seconds. Returns 0 on the (impossible) pre-epoch
/// clock — the signer treats this as "expires immediately" which is the
/// correct fail-safe.
fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod inline_tests {
    use super::*;

    #[test]
    fn wire_types_round_trip_serde_json() {
        // Sanity: JSON shape uses `end` (not end_inclusive) for url_range.
        let v = QueryReconstructionResponse {
            offset_into_first_range: 0,
            terms: vec![ReconstructionTerm {
                hash: "ab".repeat(32),
                unpacked_length: 42,
                range: ChunkRange { start: 0, end: 1 },
            }],
            fetch_info: {
                let mut m = BTreeMap::new();
                m.insert(
                    "ab".repeat(32),
                    vec![FetchInfoEntry {
                        range: ChunkRange { start: 0, end: 1 },
                        url: "https://example/x".into(),
                        url_range: UrlRange {
                            start: 0,
                            end_inclusive: 41,
                        },
                    }],
                );
                m
            },
        };
        let s = serde_json::to_string(&v).unwrap();
        // url_range must serialize as {"start": 0, "end": 41} per xet-core.
        assert!(s.contains(r#""url_range":{"start":0,"end":41}"#));
        assert!(s.contains(r#""offset_into_first_range":0"#));
        let back: QueryReconstructionResponse = serde_json::from_str(&s).unwrap();
        assert_eq!(back.terms.len(), 1);
    }
}
