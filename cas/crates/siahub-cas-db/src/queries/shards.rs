//! shards table queries + transactional reconstruction inserter.
//! option C pipeline:
//! ```text
//! BEGIN TRANSACTION
//! INSERT INTO shards (shard_hash, sia_object_id=NULL,..., pin_state='pinning')
//! ON CONFLICT (shard_hash) DO NOTHING RETURNING shard_hash
//! ├── Some(..) → fresh row → INSERT reconstruction_files + reconstruction_terms
//! └── None → dedup — caller returns {result: Exists}, no further writes
//! COMMIT
//!bytes durable in Postgres even if the next step fails (Sia-authoritative:
//! reconciler will retry the upload+pin from the parsed cache).
//! sdk.upload_and_pin(shard_bytes) → set_pin_state('pinned', Some(real_id))
//! ```
//! Runtime-checked SQL (same discipline as `queries/xorbs.rs`) so builds are
//! offline-safe with `SQLX_OFFLINE=true`. may upgrade to
//! compile-checked `query!` once `.sqlx/` is refreshed against a live DB.

use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::queries::reconstruction::{ParsedFile, ParsedTerm};
use crate::types::XorbPinState;

/// Attempt to claim the shard hash + populate its reconstruction rows.
/// One transaction covers three tables ( option C):
/// 1. `INSERT INTO shards... ON CONFLICT DO NOTHING` — atomic PK dedup.
/// 2. `INSERT INTO reconstruction_files` — bulk, one row per `ParsedFile`.
/// 3. `INSERT INTO reconstruction_terms` — bulk, one row per `ParsedTerm`.
/// Returns:
/// * `Ok(true)` on fresh insert (caller must proceed to Sia upload+pin).
/// * `Ok(false)` on dedup — the shard already exists with all its
/// reconstruction rows populated by a prior committed tx;
/// caller returns `{result: Exists}` with no Sia I/O.
/// Initial `pin_state` is `'pinning'` (schema default). `sia_object_id` is
/// left NULL; reconciler keys off the NULL + pin_state window.
pub async fn insert_shard_with_reconstruction(
    tx: &mut Transaction<'_, Postgres>,
    shard_hash: &[u8; 32],
    size_bytes: i64,
    owner_user_id: i64,
    owner_api_key_id: Uuid,
    files: &[ParsedFile],
    terms: &[ParsedTerm],
) -> Result<bool, sqlx::Error> {
    // (1) Atomic PK dedup — ON CONFLICT DO NOTHING RETURNING tells us whether
    // we won the race. If RETURNING yields a row, we own this shard_hash
    // for the remainder of the transaction.
    let row: Option<(Vec<u8>,)> = sqlx::query_as(
        "INSERT INTO shards \
           (shard_hash, sia_object_id, size_bytes, owner_user_id, owner_api_key_id) \
         VALUES ($1, NULL, $2, $3, $4) \
         ON CONFLICT (shard_hash) DO NOTHING \
         RETURNING shard_hash",
    )
    .bind(&shard_hash[..])
    .bind(size_bytes)
    .bind(owner_user_id)
    .bind(owner_api_key_id)
    .fetch_optional(&mut **tx)
    .await?;

    if row.is_none() {
        // Dedup path: the shard row + its reconstruction rows already exist
        // from a prior committed transaction. Do NOT re-insert — the
        // reconstruction_files PK would conflict anyway, and these are
        // append-only by design (.md: no DELETE).
        return Ok(false);
    }

    // (2) Bulk INSERT reconstruction_files via UNNEST of parallel BYTEA[] +
    // BIGINT[] arrays. One round-trip; O(n) rows where n = files.len.
    // In practice a shard has one file most of the time, but multi-file
    // shards are legal per the xet-core wire format.
    if !files.is_empty() {
        let file_ids: Vec<Vec<u8>> = files.iter().map(|f| f.file_id.to_vec()).collect();
        let shard_hashes: Vec<Vec<u8>> = files.iter().map(|_| shard_hash.to_vec()).collect();
        let total_sizes: Vec<i64> = files.iter().map(|f| f.total_size).collect();
        // sha256 may be absent on shards that didn't write FileMetadataExt;
        // we pass an Option<Vec<u8>> per-row and let postgres store NULL.
        let sha256s: Vec<Option<Vec<u8>>> =
            files.iter().map(|f| f.sha256.map(|h| h.to_vec())).collect();

        sqlx::query(
            "INSERT INTO reconstruction_files (file_id, shard_hash, total_size, sha256) \
             SELECT * FROM UNNEST($1::bytea[], $2::bytea[], $3::bigint[], $4::bytea[]) \
             ON CONFLICT (file_id) DO UPDATE \
                SET sha256 = COALESCE(EXCLUDED.sha256, reconstruction_files.sha256)",
        )
        .bind(&file_ids)
        .bind(&shard_hashes)
        .bind(&total_sizes)
        .bind(&sha256s)
        .execute(&mut **tx)
        .await?;
    }

    // (3) Bulk INSERT reconstruction_terms via UNNEST of nine parallel arrays.
    // All range columns remain END-EXCLUSIVE — see annotation on
    // ParsedTerm. No conversion happens here; the shard parser's
    // pre-computed values land verbatim.
    if !terms.is_empty() {
        let file_ids: Vec<Vec<u8>> = terms.iter().map(|t| t.file_id.to_vec()).collect();
        let term_indices: Vec<i32> = terms.iter().map(|t| t.term_index).collect();
        let xorb_hashes: Vec<Vec<u8>> = terms.iter().map(|t| t.xorb_hash.to_vec()).collect();
        // END-EXCLUSIVE — see.
        let xorb_starts: Vec<i64> = terms.iter().map(|t| t.xorb_start).collect();
        // END-EXCLUSIVE — see.
        let xorb_ends: Vec<i64> = terms.iter().map(|t| t.xorb_end).collect();
        // END-EXCLUSIVE — see.
        let xorb_byte_starts: Vec<i64> = terms.iter().map(|t| t.xorb_byte_start).collect();
        // END-EXCLUSIVE — see.
        let xorb_byte_ends: Vec<i64> = terms.iter().map(|t| t.xorb_byte_end).collect();
        // END-EXCLUSIVE — see.
        let unpacked_starts: Vec<i64> = terms.iter().map(|t| t.unpacked_start).collect();
        // END-EXCLUSIVE — see.
        let unpacked_ends: Vec<i64> = terms.iter().map(|t| t.unpacked_end).collect();

        sqlx::query(
            "INSERT INTO reconstruction_terms \
               (file_id, term_index, xorb_hash, \
                xorb_start, xorb_end, \
                xorb_byte_start, xorb_byte_end, \
                unpacked_start, unpacked_end) \
             SELECT * FROM UNNEST( \
               $1::bytea[], $2::int[], $3::bytea[], \
               $4::bigint[], $5::bigint[], \
               $6::bigint[], $7::bigint[], \
               $8::bigint[], $9::bigint[])",
        )
        .bind(&file_ids)
        .bind(&term_indices)
        .bind(&xorb_hashes)
        .bind(&xorb_starts)
        .bind(&xorb_ends)
        .bind(&xorb_byte_starts)
        .bind(&xorb_byte_ends)
        .bind(&unpacked_starts)
        .bind(&unpacked_ends)
        .execute(&mut **tx)
        .await?;
    }

    Ok(true)
}

/// Update `pin_state` for a shard. Mirror of `xorbs::set_pin_state`.
/// When `sia_object_id` is provided, it is written (or overwrites NULL); when
/// `None`, the existing column value is retained via `COALESCE`. Also bumps
/// `pin_attempts` and stamps `last_pin_attempt_at` so the reconciler's
/// "older than 5 minutes" backoff window applies.
pub async fn set_pin_state(
    pool: &PgPool,
    shard_hash: &[u8; 32],
    state: XorbPinState,
    sia_object_id: Option<&[u8]>,
) -> Result<(), sqlx::Error> {
    let state_text = match state {
        XorbPinState::Uploading => "uploading",
        XorbPinState::Pinning => "pinning",
        XorbPinState::Pinned => "pinned",
        XorbPinState::Orphaned => "orphaned",
    };

    sqlx::query(
        "UPDATE shards \
         SET pin_state = $1::xorb_pin_state, \
             sia_object_id = COALESCE($2, sia_object_id), \
             pin_attempts = pin_attempts + 1, \
             last_pin_attempt_at = NOW() \
         WHERE shard_hash = $3",
    )
    .bind(state_text)
    .bind(sia_object_id)
    .bind(&shard_hash[..])
    .execute(pool)
    .await?;

    Ok(())
}

/// `true` iff the shard has been successfully pinned to Sia. Mirrors
/// `xorbs::exists_pinned`. Available for future gateway lookups.
pub async fn exists_pinned(
    pool: &PgPool,
    shard_hash: &[u8; 32],
) -> Result<bool, sqlx::Error> {
    let row: Option<(i32,)> = sqlx::query_as(
        "SELECT 1 FROM shards WHERE shard_hash = $1 AND pin_state = 'pinned'",
    )
    .bind(&shard_hash[..])
    .fetch_optional(pool)
    .await?;

    Ok(row.is_some())
}

/// Batch cross-check: return the subset of `xorb_hashes` that are present
/// in `xorbs` AND in `pin_state='pinned'`. Caller computes the missing set by
/// difference. One round-trip regardless of list length.
/// Caller owns surfacing missing xorbs as the 400 `shard_missing_xorbs`
/// response body.
pub async fn which_xorbs_are_pinned(
    pool: &PgPool,
    xorb_hashes: &[[u8; 32]],
) -> Result<Vec<[u8; 32]>, sqlx::Error> {
    if xorb_hashes.is_empty() {
        return Ok(Vec::new());
    }
    let flat: Vec<Vec<u8>> = xorb_hashes.iter().map(|h| h.to_vec()).collect();

    let rows: Vec<(Vec<u8>,)> = sqlx::query_as(
        "SELECT xorb_merkle_hash FROM xorbs \
         WHERE xorb_merkle_hash = ANY($1) AND pin_state = 'pinned'",
    )
    .bind(&flat)
    .fetch_all(pool)
    .await?;

    let mut out: Vec<[u8; 32]> = Vec::with_capacity(rows.len());
    for (raw,) in rows {
        if raw.len() == 32 {
            let mut h = [0u8; 32];
            h.copy_from_slice(&raw);
            out.push(h);
        }
    }
    Ok(out)
}
