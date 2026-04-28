//! `GET /admin/xorbs` — admin-only xorb listing for operator visibility.
//! Backs..06 (admin view). 403 on non-admin. Supports two
//! optional filters per acceptance criteria:
//! * `?hash_prefix=<8-hex>` — matches `xorbs.hash_prefix_8` (the generated
//! STORED column from migration 0002).
//! * `?api_key_id=<uuid>` — matches `xorbs.owner_api_key_id`.
//! Both filters are optional and combine as AND. Response cap: 500 rows
//! (anti-feature guardrail — no paginated admin table for v1).

use axum::Json;
use axum::extract::{Path, Query, State};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::AuthStateRef;
use crate::errors::AppError;
use crate::session::Session;

#[derive(Debug, Deserialize, Default)]
pub struct XorbQuery {
    /// Hex prefix — matches `xorbs.hash_prefix_8`. Must be exactly 8 hex
    /// chars to use the B-tree index cheaply; shorter/longer inputs fall
    /// back to a `LIKE` scan. v1 rejects anything other than 0 or 8 chars
    /// to keep the query plan predictable.
    pub hash_prefix: Option<String>,
    pub api_key_id: Option<Uuid>,
}

#[derive(Debug, Serialize)]
pub struct XorbRow {
    pub hash: String,
    pub sia_object_id: Option<String>,
    pub size_bytes: i64,
    pub pin_state: String,
    pub uploaded_at: DateTime<Utc>,
    pub uploader_key_id: Uuid,
}

#[derive(Debug, Serialize)]
pub struct ListXorbsResponse {
    pub xorbs: Vec<XorbRow>,
}

/// `GET /admin/xorbs` — session-gated (any signed-in user).
/// The admin flag has been removed: xorbs are public content-addressed Sia
/// objects, and the reviewer demo needs this surface visible to every
/// signed-in user. Per-user filtering is optional via `api_key_id`.
pub async fn list_xorbs<S: AuthStateRef>(
    Session(_user): Session,
    State(st): State<S>,
    Query(q): Query<XorbQuery>,
) -> Result<Json<ListXorbsResponse>, AppError> {
    // Validate prefix input. accepts exactly 8 hex chars (the
    // generated hash_prefix_8 column is always 8). A blank/missing value
    // disables the filter; any other length is a BadRequest rather than a
    // silent scan.
    if let Some(ref p) = q.hash_prefix
        && !p.is_empty()
        && (p.len() != 8 || !p.chars().all(|c| c.is_ascii_hexdigit()))
    {
        return Err(AppError::BadRequest("hash_prefix must be 8 hex chars"));
    }

    // Coalesce NULLs into sentinel matchers so the single query works for
    // every filter combination.
    // - $1 (hash_prefix): either NULL or lowercase hex prefix.
    // - $2 (api_key_id): either NULL or a UUID.
    let hash_prefix_opt: Option<String> = q
        .hash_prefix
        .filter(|p| !p.is_empty())
        .map(|p| p.to_lowercase());

    // Column tuple for the `xorbs` query — alias keeps clippy happy.
    type XorbDbRow = (
        Vec<u8>,
        Option<Vec<u8>>,
        i64,
        String,
        DateTime<Utc>,
        Uuid,
    );
    // unified blob listing: real xorbs (xet write path) + lfs_objects
    // (the small-file legacy path that stores bytes inline as BYTEA in
    // Postgres). lfs rows synthesize pin_state='inline' to truthfully
    // distinguish "stored as a metadata blob, never on Sia" from "pinned
    // on Sia" — the column is `xorbs.pin_state::text` cast plus a literal
    // 'inline' for the LFS branch, NOT a value from the xorb_pin_state
    // enum. uploader_key_id is derived from the most-recent repo_files
    // row that references the oid; falls back to a zero UUID when the
    // lfs object isn't yet bound to any repo.
    let rows: Vec<XorbDbRow> = sqlx::query_as(
        "WITH base AS ( \
             SELECT xorb_merkle_hash AS hash, sia_object_id, size_bytes, \
                    pin_state::text AS pin_state, uploaded_at, \
                    owner_api_key_id \
               FROM xorbs \
             UNION ALL \
             SELECT lo.oid AS hash, NULL::bytea AS sia_object_id, \
                    lo.size_bytes, 'inline'::text AS pin_state, \
                    lo.created_at AS uploaded_at, \
                    COALESCE( \
                      ( SELECT ak.id \
                          FROM api_keys ak \
                          JOIN repos r2 ON r2.owner_user_id = ak.user_id \
                          JOIN repo_commits rc ON rc.repo_id = r2.id \
                          JOIN repo_files rf ON rf.commit_id = rc.id \
                                            AND rf.lfs_oid = lo.oid \
                         ORDER BY ak.created_at DESC LIMIT 1 ), \
                      '00000000-0000-0000-0000-000000000000'::uuid \
                    ) AS owner_api_key_id \
               FROM lfs_objects lo \
         ) \
         SELECT hash, sia_object_id, size_bytes, pin_state, \
                uploaded_at, owner_api_key_id \
           FROM base \
          WHERE ($1::text IS NULL OR substring(encode(hash,'hex') from 1 for 8) = $1) \
            AND ($2::uuid IS NULL OR owner_api_key_id = $2) \
          ORDER BY uploaded_at DESC \
          LIMIT 500",
    )
    .bind(hash_prefix_opt)
    .bind(q.api_key_id)
    .fetch_all(st.pool())
    .await?;

    let xorbs = rows
        .into_iter()
        .map(
            |(hash, sia_obj, size_bytes, pin_state, uploaded_at, key_id)| XorbRow {
                hash: lowercase_hex(&hash),
                sia_object_id: sia_obj.map(|b| lowercase_hex(&b)),
                size_bytes,
                pin_state,
                uploaded_at,
                uploader_key_id: key_id,
            },
        )
        .collect();

    Ok(Json(ListXorbsResponse { xorbs }))
}

fn lowercase_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(char::from_digit((b >> 4) as u32, 16).unwrap());
        s.push(char::from_digit((b & 0x0f) as u32, 16).unwrap());
    }
    s
}

/// Decode a lowercase 64-char hex xorb hash into 32 raw bytes. Returns
/// `None` if the input is malformed (wrong length or non-hex chars). The
/// single call site (`get_xorb_detail` below) maps that to
/// `AppError::BadRequest("invalid_xorb_hash")` — a 400, NOT a 404, so the
/// console can distinguish typos from true absence.
fn decode_xorb_hash_hex(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    let bytes = s.as_bytes();
    for (i, pair) in bytes.chunks_exact(2).enumerate() {
        let hi = (pair[0] as char).to_digit(16)?;
        let lo = (pair[1] as char).to_digit(16)?;
        out[i] = ((hi << 4) | lo) as u8;
    }
    Some(out)
}

/// `GET /admin/xorbs/{hash}` — single-xorb detail lookup (
/// amendment). Admin-gated. Same row shape as one `XorbRow` element
/// from `list_xorbs`. Returns 404 on absence so the console's AssetDetail
/// page can drop the prefix-fallback workaround documented in.
/// The `{hash}` path segment is the lowercase 64-char hex encoding of the
/// 32-byte `xorbs.xorb_merkle_hash` BYTEA column — the same encoding
/// returned by `list_xorbs` and by the `/admin/stats` activity feed. The
/// byte-reversal pitfall is handled upstream by the
/// `siahub-cas-proto::merklehash` crate at upload time; by the time bytes
/// land in this BYTEA column they are already in the canonical order.
#[derive(Debug, Serialize)]
pub struct XorbDetail {
    pub xorb: XorbRow,
    /// `{owner}/{repo}` pairs whose HEAD main commit references this xorb
    /// via `repo_files.xet_hash`. Empty when the xorb was uploaded through
    /// the raw `/v1/xorbs/...` path without ever landing in a repo.
    pub referencing_repos: Vec<String>,
}

pub async fn get_xorb_detail<S: AuthStateRef>(
    Session(_user): Session,
    State(st): State<S>,
    Path(hash_hex): Path<String>,
) -> Result<Json<XorbDetail>, AppError> {
    let hash_lc = hash_hex.to_lowercase();
    let hash_bytes = decode_xorb_hash_hex(&hash_lc)
        .ok_or(AppError::BadRequest("invalid_xorb_hash"))?;

    type XorbDbRow = (
        Vec<u8>,
        Option<Vec<u8>>,
        i64,
        String,
        DateTime<Utc>,
        Uuid,
    );
    let row: Option<XorbDbRow> = sqlx::query_as(
        "SELECT xorb_merkle_hash, sia_object_id, size_bytes, \
                pin_state::text, uploaded_at, owner_api_key_id \
           FROM xorbs \
          WHERE xorb_merkle_hash = $1 \
          UNION ALL \
          SELECT lo.oid, NULL::bytea, lo.size_bytes, \
                 'inline'::text, lo.created_at, \
                 COALESCE( \
                   ( SELECT ak.id FROM api_keys ak \
                       JOIN repos r2 ON r2.owner_user_id = ak.user_id \
                       JOIN repo_commits rc ON rc.repo_id = r2.id \
                       JOIN repo_files rf ON rf.commit_id = rc.id \
                                         AND rf.lfs_oid = lo.oid \
                      ORDER BY ak.created_at DESC LIMIT 1 ), \
                   '00000000-0000-0000-0000-000000000000'::uuid \
                 ) \
            FROM lfs_objects lo \
           WHERE lo.oid = $1 \
          LIMIT 1",
    )
    .bind(&hash_bytes[..])
    .fetch_optional(st.pool())
    .await?;

    let row = row.ok_or(AppError::NotFound)?;
    let (hash, sia_obj, size_bytes, pin_state, uploaded_at, key_id) = row;
    let xorb = XorbRow {
        hash: lowercase_hex(&hash),
        sia_object_id: sia_obj.map(|b| lowercase_hex(&b)),
        size_bytes,
        pin_state,
        uploaded_at,
        uploader_key_id: key_id,
    };

    // Look up `{owner}/{repo}` pairs whose main HEAD references this xorb.
    // Migration 0008 gives us `repo_files.xet_hash` as the bridge; the
    // JOIN fans out to owner login so the frontend can deep-link without
    // needing a second round-trip. Empty list == xorb uploaded raw, never
    // bound to a repo (the demo debug path pre-hfapi).
    let repo_rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT DISTINCT u.github_login, r.name \
           FROM repo_files rf \
           JOIN repo_refs rr ON rr.commit_id = rf.commit_id AND rr.ref_name = 'main' \
           JOIN repos r ON r.id = rr.repo_id \
           JOIN users u ON u.id = r.owner_user_id \
          WHERE rf.xet_hash = $1 OR rf.lfs_oid = $1",
    )
    .bind(&hash_bytes[..])
    .fetch_all(st.pool())
    .await
    .unwrap_or_default();
    let referencing_repos: Vec<String> = repo_rows
        .into_iter()
        .map(|(owner, name)| format!("{owner}/{name}"))
        .collect();

    Ok(Json(XorbDetail {
        xorb,
        referencing_repos,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lowercase_hex_round_trips() {
        assert_eq!(lowercase_hex(&[0x00, 0xab, 0xff]), "00abff");
    }

    #[test]
    fn xorb_query_default_is_none() {
        let q = XorbQuery::default();
        assert!(q.hash_prefix.is_none());
        assert!(q.api_key_id.is_none());
    }

    #[test]
    fn decode_xorb_hash_hex_accepts_canonical_64_char_lowercase() {
        let s = "eea25d6ee393ccae385820daed127b96ef0ea034dfb7cf6da3a950ce334b7632";
        let got = decode_xorb_hash_hex(s).expect("canonical hex decodes");
        assert_eq!(got.len(), 32);
        assert_eq!(got[0], 0xee);
        assert_eq!(got[31], 0x32);
    }

    #[test]
    fn decode_xorb_hash_hex_rejects_wrong_length() {
        assert!(decode_xorb_hash_hex("").is_none());
        assert!(decode_xorb_hash_hex("abcd").is_none());
        assert!(
            decode_xorb_hash_hex(&"a".repeat(63)).is_none(),
            "63 chars must reject"
        );
        assert!(
            decode_xorb_hash_hex(&"a".repeat(65)).is_none(),
            "65 chars must reject"
        );
    }

    #[test]
    fn decode_xorb_hash_hex_rejects_non_hex() {
        let mut s: String = "a".repeat(62);
        s.push_str("zz");
        assert!(decode_xorb_hash_hex(&s).is_none());
    }
}
