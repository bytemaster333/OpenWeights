//! Shard binary parsing + P18/ validation ( Task 1).
//! Pitfall ownership:
//! - **P18** (shard cross-check): `parse_and_validate` extracts the set of
//! referenced xorb hashes via [`ParsedShard::referenced_xorb_hashes`]. The
//! handler cross-checks these against `xorbs.pin_state='pinned'` BEFORE
//! opening any DB transaction.
//! - **P19** (version gating): `parse_and_validate` rejects any shard where
//! `MDBShardFileHeader.version != 2` OR `MDBShardFileFooter.version != 1`.
//! On mismatch we emit a structured log (the Prometheus metric wiring is
//! deferred to — rationale in SUMMARY).
//! Pre-computed byte offsets ( / RESEARCH §2.5): reconstruction queries
//! MUST NOT re-fetch xorb footers at query time. This module computes
//! `xorb_byte_start`/`xorb_byte_end` at shard-upload time from the shard's own
//! XorbInfo section (each `XorbChunkSequenceEntry` carries
//! `chunk_byte_range_start` + `unpacked_segment_bytes`) and persists them into
//! `reconstruction_terms` via `queries/shards.rs::insert_shard_with_reconstruction`.
//! The entire module is `pub(crate)` visible to the handler; the handler is
//! the only caller. `parse_and_validate` is a pure function; the Postgres
//! cross-check happens in the handler (it needs a pool).

use std::collections::HashMap;
use std::io::Cursor;

use siahub_cas_db::queries::reconstruction::{ParsedFile, ParsedTerm};
use siahub_cas_proto::metadata_shard::shard_format::MDBShardInfo;
use siahub_cas_proto::metadata_shard::xorb_structs::MDBXorbInfo;
use siahub_cas_proto::metadata_shard::{MDBShardFileFooter, MDBShardFileHeader};

/// Locked wire versions (PITFALL ). Any other value → 400.
pub const EXPECTED_HEADER_VERSION: u64 = 2;
pub const EXPECTED_FOOTER_VERSION: u64 = 1;

/// Fully-parsed shard content.
#[derive(Debug, Clone)]
pub struct ParsedShard {
    /// Informational — always `EXPECTED_HEADER_VERSION` after validation.
    pub header_version: u64,
    /// Informational — always `EXPECTED_FOOTER_VERSION` after validation.
    pub footer_version: u64,
    /// De-duplicated list of referenced xorb merkle hashes (one entry per
    /// unique xorb the shard's file terms cite). The handler cross-checks
    /// these against `xorbs.pin_state='pinned'` (PITFALL ).
    pub referenced_xorb_hashes: Vec<[u8; 32]>,
    /// One DTO per file entry in the shard's FileInfo section. Persisted
    /// verbatim into `reconstruction_files`.
    pub files: Vec<ParsedFile>,
    /// One DTO per file term, in the order xet-core serializes them. Persisted
    /// into `reconstruction_terms`. All range fields are END-EXCLUSIVE.
    pub terms: Vec<ParsedTerm>,
}

/// Parser errors that map at the handler boundary to `AppError` variants.
/// Mapping (see `handlers::shards` for the actual match):
/// * `HeaderVersion(_)` / `FooterVersion(_)` → `AppError::ShardVersionUnsupported` (400)
/// * everything else → `AppError::BadRequest("malformed_shard")` (400)
#[derive(Debug, thiserror::Error)]
pub enum ShardParseError {
    #[error("malformed shard: {0}")]
    Malformed(String),

    #[error("unsupported shard header version: got {0}, expected {}", EXPECTED_HEADER_VERSION)]
    HeaderVersion(u64),

    #[error("unsupported shard footer version: got {0}, expected {}", EXPECTED_FOOTER_VERSION)]
    FooterVersion(u64),

    #[error("shard declares no files")]
    EmptyFileList,

    #[error(
        "inconsistent term range in file {file_id_hex}, term {term_index}: {reason}"
    )]
    InconsistentTermRange {
        file_id_hex: String,
        term_index: i32,
        reason: &'static str,
    },
}

/// Parse-and-validate a shard body.
/// Side effects: `tracing::warn!` on version mismatch with `header_version`
/// and `footer_version` fields — hoists this into a Prometheus
/// counter `siahub_cas_shard_version_rejected_total{header_version,footer_version}`.
/// Bounds: caller MUST cap the body (handler uses 16 MiB). Unbounded input is
/// a DoS primitive; this function does not re-bound.
pub fn parse_and_validate(bytes: &[u8]) -> Result<ParsedShard, ShardParseError> {
    // Minimum-size guard — must at least fit the fixed-size header + footer.
    const MIN_SHARD_BYTES: usize = 48 /* header*/ + 200 /* footer*/;
    if bytes.len() < MIN_SHARD_BYTES {
        return Err(ShardParseError::Malformed(format!(
            "shard too small: {} bytes (min {MIN_SHARD_BYTES})",
            bytes.len()
        )));
    }

    // (1) Header parse — magic-number + version. .
    let header = {
        let mut cur = Cursor::new(bytes);
        MDBShardFileHeader::deserialize(&mut cur)
            .map_err(|e| ShardParseError::Malformed(format!("header: {e}")))?
    };
    if header.version != EXPECTED_HEADER_VERSION {
        //structured log so can convert into a Prom counter
        // without changing this module's surface.
        tracing::warn!(
            header_version = header.version,
            footer_version = 0u64, // footer not yet parsed
            "shard version rejected (header)"
        );
        return Err(ShardParseError::HeaderVersion(header.version));
    }

    // (2) Footer parse — last MDB_SHARD_FOOTER_SIZE bytes of the body. The
    // xet-core crate's `MDBShardFileFooter::deserialize` internally
    // validates `version == MDB_SHARD_FOOTER_VERSION (== 1)` and returns
    // `CoreError::ShardVersion` on mismatch. We reject the same way.
    // Footer size is hard-coded to 200 bytes — size of
    // `MDBShardFileFooter` per the xet-core 1.5.1 source. The shard's
    // `header.footer_size` field also encodes the size; the crate itself
    // relies on its own compile-time constant. We follow suit.
    const FOOTER_BYTES: usize = 200;
    let footer_start = bytes
        .len()
        .checked_sub(FOOTER_BYTES)
        .ok_or_else(|| ShardParseError::Malformed("cannot locate footer".into()))?;
    // Try to parse; on any error, try to extract the leading u64 version for
    // the log line so operators can see a version mismatch even if a later
    // field is also bad.
    let footer = {
        let mut cur = Cursor::new(&bytes[footer_start..]);
        match MDBShardFileFooter::deserialize(&mut cur) {
            Ok(f) => f,
            Err(e) => {
                // Extract the footer version word directly from the first 8
                // bytes (little-endian u64). If it is NOT 1, report with
                // the real value; otherwise report a generic malformed footer.
                let raw_version = {
                    let mut v = [0u8; 8];
                    v.copy_from_slice(&bytes[footer_start..footer_start + 8]);
                    u64::from_le_bytes(v)
                };
                if raw_version != EXPECTED_FOOTER_VERSION {
                    tracing::warn!(
                        header_version = header.version,
                        footer_version = raw_version,
                        "shard version rejected (footer)"
                    );
                    return Err(ShardParseError::FooterVersion(raw_version));
                }
                return Err(ShardParseError::Malformed(format!("footer: {e}")));
            }
        }
    };
    if footer.version != EXPECTED_FOOTER_VERSION {
        tracing::warn!(
            header_version = header.version,
            footer_version = footer.version,
            "shard version rejected (footer)"
        );
        return Err(ShardParseError::FooterVersion(footer.version));
    }

    // (3) Full-body parse via MDBShardInfo. Walks both the FileInfo and
    // XorbInfo sections using seek offsets from the already-validated
    // footer — this is the same path xet-core's own shard readers take.
    let mut seek_reader = Cursor::new(bytes);
    let shard_info = MDBShardInfo::load_from_reader(&mut seek_reader)
        .map_err(|e| ShardParseError::Malformed(format!("shard load: {e}")))?;

    // (4) Read xorb-info sections — these give us per-chunk byte offsets
    // within each xorb. We need them to pre-compute `xorb_byte_start` /
    // `xorb_byte_end` per term. Indexed by xorb_hash for O(1) term
    // lookup below.
    let xorb_infos: Vec<MDBXorbInfo> = shard_info
        .read_all_xorb_blocks_full(&mut seek_reader)
        .map_err(|e| ShardParseError::Malformed(format!("xorb info: {e}")))?;
    let mut xorb_index: HashMap<[u8; 32], Vec<(u32, u32)>> = HashMap::with_capacity(xorb_infos.len());
    for xi in &xorb_infos {
        let hash: [u8; 32] = xi.metadata.xorb_hash.into();
        // One entry per chunk: (byte_range_start, unpacked_segment_bytes).
        // Together they give the END-EXCLUSIVE (start..start+len) byte range
        // of that chunk inside the serialized xorb — .
        let chunks: Vec<(u32, u32)> = xi
            .chunks
            .iter()
            .map(|c| (c.chunk_byte_range_start, c.unpacked_segment_bytes))
            .collect();
        // De-dup by xorb_hash: xet-core shards typically carry each xorb
        // exactly once; on the off chance two entries appear for the same
        // hash, keep the first (same bytes → same offsets).
        xorb_index.entry(hash).or_insert(chunks);
    }

    // (5) Read file-info sections. One ParsedFile + one-or-more ParsedTerms
    // per file. The term_index is a serial within the file, starting at
    // zero, matching `reconstruction_terms (file_id, term_index)` PK.
    let file_infos = shard_info
        .read_all_file_info_sections(&mut seek_reader)
        .map_err(|e| ShardParseError::Malformed(format!("file info: {e}")))?;

    if file_infos.is_empty() {
        return Err(ShardParseError::EmptyFileList);
    }

    let mut files: Vec<ParsedFile> = Vec::with_capacity(file_infos.len());
    let mut terms: Vec<ParsedTerm> = Vec::new();
    let mut referenced_set: HashMap<[u8; 32], ()> = HashMap::new();

    for fi in &file_infos {
        let file_id: [u8; 32] = fi.metadata.file_hash.into();
        let total_size: i64 = fi
            .segments
            .iter()
            .map(|s| s.unpacked_segment_bytes as i64)
            .sum();
        files.push(ParsedFile { file_id, total_size });

        // Track the running unpacked cursor for this file so we can produce
        // END-EXCLUSIVE `unpacked_start` / `unpacked_end` offsets per term
        //. xet-core serializes terms in order, so a simple accumulator
        // matches the wire semantics.
        let mut unpacked_cursor: i64 = 0;
        for (idx, seg) in fi.segments.iter().enumerate() {
            let xorb_hash: [u8; 32] = seg.xorb_hash.into();
            // Record for the cross-check.
            referenced_set.insert(xorb_hash, ());

            // Chunk-index range — END-EXCLUSIVE per .
            let xorb_start = seg.chunk_index_start as i64;
            let xorb_end = seg.chunk_index_end as i64;
            if xorb_start >= xorb_end {
                return Err(ShardParseError::InconsistentTermRange {
                    file_id_hex: hex_of(&file_id),
                    term_index: idx as i32,
                    reason: "xorb_start must be strictly less than xorb_end",
                });
            }

            // Byte-range inside the xorb: look up the per-chunk table; sum
            // `chunk_byte_range_start` of the FIRST chunk in the segment and
            // (start+len) of the LAST chunk. If the xorb is not in our index
            // (shard is malformed — file refers to a xorb the shard does not
            // describe), the cross-check against the DB would still trip
            // later, but we reject early with a clearer reason.
            let chunks = xorb_index.get(&xorb_hash).ok_or_else(|| {
                ShardParseError::InconsistentTermRange {
                    file_id_hex: hex_of(&file_id),
                    term_index: idx as i32,
                    reason: "xorb referenced but not present in shard XorbInfo",
                }
            })?;
            let first_idx = xorb_start as usize;
            // END-EXCLUSIVE — last chunk actually referenced is at xorb_end-1.
            let last_idx = (xorb_end - 1) as usize;
            if last_idx >= chunks.len() {
                return Err(ShardParseError::InconsistentTermRange {
                    file_id_hex: hex_of(&file_id),
                    term_index: idx as i32,
                    reason: "xorb_end exceeds chunk count in XorbInfo",
                });
            }
            let (first_start, _) = chunks[first_idx];
            let (last_start, last_len) = chunks[last_idx];
            // END-EXCLUSIVE byte offsets — see .
            let xorb_byte_start = first_start as i64;
            let xorb_byte_end = last_start as i64 + last_len as i64;
            if xorb_byte_start >= xorb_byte_end {
                return Err(ShardParseError::InconsistentTermRange {
                    file_id_hex: hex_of(&file_id),
                    term_index: idx as i32,
                    reason: "xorb_byte_start must be strictly less than xorb_byte_end",
                });
            }

            // Unpacked byte range — END-EXCLUSIVE.
            let unpacked_start = unpacked_cursor;
            let unpacked_end = unpacked_cursor + seg.unpacked_segment_bytes as i64;
            unpacked_cursor = unpacked_end;

            terms.push(ParsedTerm {
                file_id,
                term_index: idx as i32,
                xorb_hash,
                xorb_start,
                xorb_end,
                xorb_byte_start,
                xorb_byte_end,
                unpacked_start,
                unpacked_end,
            });
        }
    }

    // Stable iteration order — HashMap randomizes, which would make error
    // bodies flaky across runs. Sort for determinism before surfacing.
    let mut referenced_xorb_hashes: Vec<[u8; 32]> = referenced_set.into_keys().collect();
    referenced_xorb_hashes.sort();

    Ok(ParsedShard {
        header_version: header.version,
        footer_version: footer.version,
        referenced_xorb_hashes,
        files,
        terms,
    })
}

/// Hex-encode a 32-byte hash for inclusion in error messages ONLY. This is
/// NOT on the wire-hash path; the handler uses `MerkleHash::hex` (from the
/// xet-core crate codec) for everything facing the client.
fn hex_of(h: &[u8; 32]) -> String {
    // Minimal inline hex — 64-char lowercase. Used only in parser error
    // strings that land in 400 response bodies; the path hash codec is NOT
    // this function (that's `MerkleHash::hex`).
    let mut s = String::with_capacity(64);
    for b in h {
        use std::fmt::Write;
        let _ = write!(s, "{b:02x}");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    // A zero-filled buffer is guaranteed to fail — magic number does not
    // match the MDB_SHARD_HEADER_TAG.
    #[test]
    fn rejects_obviously_non_shard_bytes() {
        let buf = vec![0u8; 2048];
        let err = parse_and_validate(&buf).expect_err("non-shard bytes must fail");
        assert!(matches!(err, ShardParseError::Malformed(_)));
    }

    #[test]
    fn rejects_too_small_body() {
        let buf = vec![0u8; 16];
        let err = parse_and_validate(&buf).expect_err("too-small body must fail");
        match err {
            ShardParseError::Malformed(msg) => {
                assert!(msg.contains("shard too small"), "got: {msg}");
            }
            other => panic!("expected Malformed, got {other:?}"),
        }
    }
}
