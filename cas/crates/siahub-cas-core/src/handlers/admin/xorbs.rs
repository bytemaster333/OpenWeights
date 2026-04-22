//! `GET /admin/xorbs` — admin-only xorb listing for operator visibility.
//!
//! Backs CONSOLE-03..06 (admin view). 403 on non-admin. Supports two
//! optional filters per 04-06 acceptance criteria:
//!
//!   * `?hash_prefix=<8-hex>` — matches `xorbs.hash_prefix_8` (the generated
//!     STORED column from migration 0002).
//!   * `?api_key_id=<uuid>` — matches `xorbs.owner_api_key_id`.
//!
//! Both filters are optional and combine as AND. Response cap: 500 rows
//! (anti-feature guardrail — no paginated admin table for v1).

use axum::Json;
use axum::extract::{Query, State};
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

/// `GET /admin/xorbs` — admin-gated.
pub async fn list_xorbs<S: AuthStateRef>(
    Session(user): Session,
    State(st): State<S>,
    Query(q): Query<XorbQuery>,
) -> Result<Json<ListXorbsResponse>, AppError> {
    if !user.is_admin {
        return Err(AppError::Forbidden);
    }

    // Validate prefix input. 04-06 accepts exactly 8 hex chars (the
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
    //
    // - $1 (hash_prefix): either NULL or lowercase hex prefix.
    // - $2 (api_key_id):  either NULL or a UUID.
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
    let rows: Vec<XorbDbRow> = sqlx::query_as(
        "SELECT xorb_merkle_hash, sia_object_id, size_bytes, \
                pin_state::text, uploaded_at, owner_api_key_id \
           FROM xorbs \
          WHERE ($1::text IS NULL OR hash_prefix_8 = $1) \
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
}
