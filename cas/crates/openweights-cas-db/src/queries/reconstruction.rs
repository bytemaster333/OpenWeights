//! reconstruction_files + reconstruction_terms read-side queries
//! AND write-side DTOs shared with `queries/shards.rs`.
//! Zero Sia I/O ( option C — Postgres is the parsed cache of shard
//! content; Sia holds the durable shard bytes). 's handler +
//! coalescer consume these queries; the single `- 1` end-exclusive →
//! end-inclusive conversion lives in `openweights_cas_core::coalesce` and nowhere
//! else.
//! Schema reference: `cas/migrations/0002_xorbs_shards.sql`.
//! Invariant (PITFALL ): every byte/chunk offset column in this module is
//! **END-EXCLUSIVE**. Rust field doc comments below restate this at each
//! field. Any code that needs end-inclusive form MUST go through
//! `coalesce_terms_by_xorb`.
//! Runtime-checked SQL (not `sqlx::query_as!`) is used so `cargo build`
//! succeeds with `SQLX_OFFLINE=true` without a regenerated `.sqlx/` cache.
//! or may upgrade these to compile-checked queries once the
//! offline cache is regenerated against a live database.
//! write-side DTOs (`ParsedFile`, `ParsedTerm`) live in THIS module
//! so the shard parser (in `openweights-cas-core`) and the transactional insert (in
//! `queries/shards.rs`, this crate) can both depend on them without creating a
//! cycle `core → db → core`. Only the shard handler constructs them; the
//! DB layer consumes them read-only.

use std::collections::HashMap;

use sqlx::PgPool;

/// DTO: one file entry parsed from a shard's FileInfo section.
/// `file_id` is the 32-byte xet-core file merkle hash (raw bytes, NOT hex).
/// `total_size` is the sum of `unpacked_segment_bytes` across the file's
/// terms — matches `reconstruction_files.total_size` column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedFile {
    pub file_id: [u8; 32],
    pub total_size: i64,
    /// optional — the file content sha256 stored in the shard's
    /// FileMetadataExt, decoded via DataHash::hex() (xet's per-8-byte-group
    /// LE encoding). matches the OID hf_hub sends in commit lfsFile entries.
    /// when None, the shard didn't carry the metadata_ext block.
    pub sha256: Option<[u8; 32]>,
}

/// DTO: one row destined for `reconstruction_terms`.
/// Range convention (PITFALL ):
/// - `xorb_start.. xorb_end` — chunk-index range, END-EXCLUSIVE.
/// - `xorb_byte_start.. xorb_byte_end` — pre-computed byte offsets inside
///   the xorb, END-EXCLUSIVE. reads these verbatim — no Sia
///   footer re-fetch at reconstruction time.
/// - `unpacked_start.. unpacked_end` — byte offsets inside the
///   reconstructed file, END-EXCLUSIVE.
///   The single `- 1` conversion to end-INCLUSIVE byte range lives in
///   `coalesce_terms_by_xorb`. The schema stays end-exclusive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedTerm {
    pub file_id: [u8; 32],
    pub term_index: i32,
    pub xorb_hash: [u8; 32],
    /// END-EXCLUSIVE — see.
    pub xorb_start: i64,
    /// END-EXCLUSIVE — see.
    pub xorb_end: i64,
    /// END-EXCLUSIVE — see.
    pub xorb_byte_start: i64,
    /// END-EXCLUSIVE — see.
    pub xorb_byte_end: i64,
    /// END-EXCLUSIVE — see.
    pub unpacked_start: i64,
    /// END-EXCLUSIVE — see.
    pub unpacked_end: i64,
}

/// One row from `reconstruction_files`. `total_size` is the byte length of
/// the reconstructed file (sum of all term unpacked ranges for an
/// un-truncated, un-compressed reconstruction).
#[derive(Debug, Clone)]
pub struct ReconstructionFile {
    pub file_id: [u8; 32],
    pub total_size: i64,
}

/// One `reconstruction_terms` row. All `*_start`/`*_end` pairs are
/// **END-EXCLUSIVE** per the migration comment in `0002_xorbs_shards.sql`.
/// Per-field semantics (repeat the invariant at every site — PITFALL ):
/// * `xorb_hash`: 32-byte merkle hash of the xorb (BYTEA in DB).
/// * `xorb_start..xorb_end`: chunk-index range within the xorb
///   (END-EXCLUSIVE chunk indices). Used by xet-core's `fetch_info`
///   `range` block (also END-EXCLUSIVE).
/// * `xorb_byte_start..xorb_byte_end`: pre-computed byte offsets within the
///   serialized xorb (END-EXCLUSIVE bytes). These are the inputs to
///   `coalesce_terms_by_xorb` which is the ONLY place that converts to
///   END-INCLUSIVE HTTP Range byte offsets.
/// * `unpacked_start..unpacked_end`: byte offsets within the reconstructed
///   output file (END-EXCLUSIVE bytes).
#[derive(Debug, Clone)]
pub struct Term {
    pub xorb_hash: [u8; 32],
    /// END-EXCLUSIVE chunk index in the xorb.
    pub xorb_start: i64,
    /// END-EXCLUSIVE chunk index in the xorb.
    pub xorb_end: i64,
    /// END-EXCLUSIVE byte offset in the serialized xorb.
    pub xorb_byte_start: i64,
    /// END-EXCLUSIVE byte offset in the serialized xorb.
    pub xorb_byte_end: i64,
    /// END-EXCLUSIVE byte offset in the reconstructed output file.
    pub unpacked_start: i64,
    /// END-EXCLUSIVE byte offset in the reconstructed output file.
    pub unpacked_end: i64,
}

/// Assembled reconstruction row — the file header + ordered term list.
#[derive(Debug, Clone)]
pub struct ReconstructionRow {
    pub file: ReconstructionFile,
    pub terms: Vec<Term>,
}

/// Fetch a single reconstruction by `file_id`. Returns `Ok(None)` if either
/// the file row is absent OR every referenced xorb is in a non-pinned
/// state — in the latter case we surface the reconstruction as "not found"
/// rather than a confusing 503/partial response (: non-pinned xorbs must
/// not appear in fetch_info).
/// The JOIN filter `xorbs.pin_state = 'pinned'` excludes terms whose xorb
/// has not yet been durably pinned to Sia. If ANY term's xorb is non-pinned,
/// that term is dropped from the result; if ALL terms drop, the caller sees
/// `Ok(None)` because `terms.is_empty` returns no file row (see query
/// below).
pub async fn get_reconstruction(
    pool: &PgPool,
    file_id: &[u8; 32],
) -> Result<Option<ReconstructionRow>, sqlx::Error> {
    // Row shape: (file.total_size, term fields...) — file_id is echoed to
    // avoid reshaping on Rust side but is the same for every row.
    type RowT = (
        i64,     // f.total_size
        Vec<u8>, // t.xorb_hash
        i64,     // t.xorb_start
        i64,     // t.xorb_end
        i64,     // t.xorb_byte_start
        i64,     // t.xorb_byte_end
        i64,     // t.unpacked_start
        i64,     // t.unpacked_end
    );

    let rows: Vec<RowT> = sqlx::query_as(
        "SELECT f.total_size, \
                t.xorb_hash, \
                t.xorb_start, t.xorb_end, \
                t.xorb_byte_start, t.xorb_byte_end, \
                t.unpacked_start, t.unpacked_end \
         FROM reconstruction_files f \
         JOIN reconstruction_terms t ON t.file_id = f.file_id \
         JOIN xorbs x ON x.xorb_merkle_hash = t.xorb_hash AND x.pin_state = 'pinned' \
         WHERE f.file_id = $1 \
         ORDER BY t.term_index ASC",
    )
    .bind(&file_id[..])
    .fetch_all(pool)
    .await?;

    if rows.is_empty() {
        return Ok(None);
    }

    let total_size = rows[0].0;
    let terms = rows
        .into_iter()
        .map(|r| Term {
            xorb_hash: into_32(&r.1),
            //all columns here are END-EXCLUSIVE (see migration comment).
            xorb_start: r.2,
            xorb_end: r.3,
            xorb_byte_start: r.4,
            xorb_byte_end: r.5,
            unpacked_start: r.6,
            unpacked_end: r.7,
        })
        .collect();

    Ok(Some(ReconstructionRow {
        file: ReconstructionFile {
            file_id: *file_id,
            total_size,
        },
        terms,
    }))
}

/// Batch fetch. Returns a map keyed by `file_id`; missing file_ids are simply
/// absent from the map (caller decides how to surface "missing" in the wire
/// response — / xet-core `batch_get_reconstruction` semantics).
/// Implementation: one round-trip via `file_id = ANY($1)` with the same
/// pinned-only JOIN filter as `get_reconstruction`. Rows are bucketed in Rust
/// to preserve term ordering per file.
pub async fn get_reconstructions_batch(
    pool: &PgPool,
    file_ids: &[[u8; 32]],
) -> Result<HashMap<[u8; 32], ReconstructionRow>, sqlx::Error> {
    if file_ids.is_empty() {
        return Ok(HashMap::new());
    }

    // `BYTEA[]` binding: flatten to Vec<Vec<u8>> so sqlx encodes as
    // Postgres BYTEA[] (fred's Postgres does not accept &[[u8;32]] directly).
    let ids_flat: Vec<Vec<u8>> = file_ids.iter().map(|b| b.to_vec()).collect();

    type RowT = (
        Vec<u8>, // f.file_id
        i64,     // f.total_size
        i32,     // t.term_index (preserved implicitly via ORDER BY but bucketed deterministically)
        Vec<u8>, // t.xorb_hash
        i64,
        i64,
        i64,
        i64,
        i64,
        i64,
    );

    let rows: Vec<RowT> = sqlx::query_as(
        "SELECT f.file_id, f.total_size, t.term_index, \
                t.xorb_hash, \
                t.xorb_start, t.xorb_end, \
                t.xorb_byte_start, t.xorb_byte_end, \
                t.unpacked_start, t.unpacked_end \
         FROM reconstruction_files f \
         JOIN reconstruction_terms t ON t.file_id = f.file_id \
         JOIN xorbs x ON x.xorb_merkle_hash = t.xorb_hash AND x.pin_state = 'pinned' \
         WHERE f.file_id = ANY($1) \
         ORDER BY f.file_id, t.term_index ASC",
    )
    .bind(&ids_flat)
    .fetch_all(pool)
    .await?;

    let mut out: HashMap<[u8; 32], ReconstructionRow> = HashMap::new();
    for r in rows {
        let fid = into_32(&r.0);
        let term = Term {
            xorb_hash: into_32(&r.3),
            //all END-EXCLUSIVE.
            xorb_start: r.4,
            xorb_end: r.5,
            xorb_byte_start: r.6,
            xorb_byte_end: r.7,
            unpacked_start: r.8,
            unpacked_end: r.9,
        };
        let entry = out.entry(fid).or_insert_with(|| ReconstructionRow {
            file: ReconstructionFile {
                file_id: fid,
                total_size: r.1,
            },
            terms: Vec::new(),
        });
        entry.terms.push(term);
    }

    Ok(out)
}

/// Defensive BYTEA → [u8; 32] converter. The schema constrains these columns
/// to 32-byte hashes; a wrong-length row indicates DB corruption, which we
/// surface as a zeroed hash rather than panicking — downstream coalescing
/// will then group all such rows together but the resulting fetch_info entry
/// will not reference a real xorb. In practice the constraint never trips
/// because `xorb_hash` FKs to `xorbs(xorb_merkle_hash)` which is itself a
/// BYTEA PRIMARY KEY without explicit length constraint, so a bad value
/// would have failed at xorb insert time.
fn into_32(src: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    let n = src.len().min(32);
    out[..n].copy_from_slice(&src[..n]);
    out
}
