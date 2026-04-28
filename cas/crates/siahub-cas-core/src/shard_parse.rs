//! shard binary parsing.
//!
//! hf_xet uploads "footer-stripped" shards: the client truncates the shard
//! at file_lookup_offset and rewrites the header with footer_size=0 before
//! POSTing to /shards. the on-the-wire format is:
//!
//!   [header 48B, footer_size=0]
//!   [file info entries, terminated by FileDataSequenceHeader::bookend()]
//!   [xorb info entries, terminated by XorbChunkSequenceHeader::bookend()]
//!   [eof — no footer, no lookup tables]
//!
//! we parse this sequentially. legacy "complete" shards with footer_size>0
//! still parse via the same code path: the bookends are written either way.

use std::collections::HashMap;
use std::io::Cursor;

use siahub_cas_db::queries::reconstruction::{ParsedFile, ParsedTerm};
use siahub_cas_proto::metadata_shard::file_structs::MDBFileInfo;
use siahub_cas_proto::metadata_shard::xorb_structs::MDBXorbInfo;
use siahub_cas_proto::metadata_shard::MDBShardFileHeader;

pub const EXPECTED_HEADER_VERSION: u64 = 2;

/// fully-parsed shard content.
#[derive(Debug, Clone)]
pub struct ParsedShard {
    pub header_version: u64,
    pub footer_version: u64,
    /// de-duplicated list of referenced xorb merkle hashes.
    pub referenced_xorb_hashes: Vec<[u8; 32]>,
    pub files: Vec<ParsedFile>,
    pub terms: Vec<ParsedTerm>,
}

#[derive(Debug, thiserror::Error)]
pub enum ShardParseError {
    #[error("malformed shard: {0}")]
    Malformed(String),

    #[error("unsupported shard header version: got {0}, expected 2")]
    HeaderVersion(u64),

    /// kept for backward compat with the handler's match arms; no longer
    /// emitted because the wire format has no footer.
    #[error("unsupported shard footer version: got {0}")]
    FooterVersion(u64),

    #[error("shard declares no files")]
    EmptyFileList,

    #[error("inconsistent term range in file {file_id_hex}, term {term_index}: {reason}")]
    InconsistentTermRange {
        file_id_hex: String,
        term_index: i32,
        reason: &'static str,
    },
}

/// parse-and-validate a shard body. handles both footer-stripped (hf_xet
/// upload format) and complete (legacy) shards transparently — both write
/// bookend-terminated file/xorb sections that we read sequentially.
pub fn parse_and_validate(bytes: &[u8]) -> Result<ParsedShard, ShardParseError> {
    const HEADER_SIZE: usize = 48;
    if bytes.len() < HEADER_SIZE {
        return Err(ShardParseError::Malformed(format!(
            "shard too small: {} bytes (min {HEADER_SIZE})",
            bytes.len()
        )));
    }

    // (1) header — magic tag + version + footer_size.
    let mut cur = Cursor::new(bytes);
    let header = MDBShardFileHeader::deserialize(&mut cur)
        .map_err(|e| ShardParseError::Malformed(format!("header: {e}")))?;
    if header.version != EXPECTED_HEADER_VERSION {
        return Err(ShardParseError::HeaderVersion(header.version));
    }

    // (2) file info section: read MDBFileInfo entries sequentially until
    // bookend (returns None). cur position advances through the body.
    let mut file_infos: Vec<MDBFileInfo> = Vec::new();
    loop {
        match MDBFileInfo::deserialize(&mut cur) {
            Ok(Some(fi)) => file_infos.push(fi),
            Ok(None) => break, // bookend hit
            Err(e) => {
                return Err(ShardParseError::Malformed(format!(
                    "file info: {e}"
                )));
            }
        }
    }

    if file_infos.is_empty() {
        return Err(ShardParseError::EmptyFileList);
    }

    // (3) xorb info section: same pattern.
    let mut xorb_infos: Vec<MDBXorbInfo> = Vec::new();
    loop {
        match MDBXorbInfo::deserialize(&mut cur) {
            Ok(Some(xi)) => xorb_infos.push(xi),
            Ok(None) => break,
            Err(e) => {
                return Err(ShardParseError::Malformed(format!(
                    "xorb info: {e}"
                )));
            }
        }
    }

    // (4) build the per-xorb chunk index for byte-range pre-computation.
    let mut xorb_index: HashMap<[u8; 32], Vec<(u32, u32)>> =
        HashMap::with_capacity(xorb_infos.len());
    for xi in &xorb_infos {
        let hash: [u8; 32] = xi.metadata.xorb_hash.into();
        let chunks: Vec<(u32, u32)> = xi
            .chunks
            .iter()
            .map(|c| (c.chunk_byte_range_start, c.unpacked_segment_bytes))
            .collect();
        xorb_index.entry(hash).or_insert(chunks);
    }

    // (5) walk the file infos to produce ParsedFile + ParsedTerm rows.
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
        // FileMetadataExt.sha256 stores the file content sha256 in xet's
        // per-8-byte-group LE encoding. .hex() produces the canonical
        // sha256 string; we re-decode that into raw bytes so the BYTEA
        // column matches what hf_hub commit sends as `oid`.
        let sha256: Option<[u8; 32]> = fi.metadata_ext.as_ref().and_then(|ext| {
            let hex = ext.sha256.hex();
            decode_hex32(&hex)
        });
        files.push(ParsedFile {
            file_id,
            total_size,
            sha256,
        });

        let mut unpacked_cursor: i64 = 0;
        for (idx, seg) in fi.segments.iter().enumerate() {
            let xorb_hash: [u8; 32] = seg.xorb_hash.into();
            referenced_set.insert(xorb_hash, ());

            let xorb_start = seg.chunk_index_start as i64;
            let xorb_end = seg.chunk_index_end as i64;
            if xorb_start >= xorb_end {
                return Err(ShardParseError::InconsistentTermRange {
                    file_id_hex: hex_of(&file_id),
                    term_index: idx as i32,
                    reason: "xorb_start must be strictly less than xorb_end",
                });
            }

            // chunk byte ranges are only known for xorbs whose XorbInfo is in
            // THIS shard. hf_xet may reference xorbs from prior upload
            // sessions (chunk dedup) without re-listing them — accept those
            // and leave byte ranges as 0,0. xet_file_serve iterates by chunk
            // index anyway and never reads these byte columns.
            let (xorb_byte_start, xorb_byte_end) = match xorb_index.get(&xorb_hash) {
                Some(chunks) => {
                    let first_idx = xorb_start as usize;
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
                    let bs = first_start as i64;
                    let be = last_start as i64 + last_len as i64;
                    if bs >= be {
                        return Err(ShardParseError::InconsistentTermRange {
                            file_id_hex: hex_of(&file_id),
                            term_index: idx as i32,
                            reason: "xorb_byte_start must be strictly less than xorb_byte_end",
                        });
                    }
                    (bs, be)
                }
                None => (0, 0), // external xorb — chunk metadata recorded by an earlier shard
            };

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

    let mut referenced_xorb_hashes: Vec<[u8; 32]> = referenced_set.into_keys().collect();
    referenced_xorb_hashes.sort();

    Ok(ParsedShard {
        header_version: header.version,
        footer_version: 0, // no footer in the wire format
        referenced_xorb_hashes,
        files,
        terms,
    })
}

fn decode_hex32(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, pair) in s.as_bytes().chunks_exact(2).enumerate() {
        let hi = (pair[0] as char).to_digit(16)?;
        let lo = (pair[1] as char).to_digit(16)?;
        out[i] = ((hi << 4) | lo) as u8;
    }
    Some(out)
}

fn hex_of(h: &[u8; 32]) -> String {
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
