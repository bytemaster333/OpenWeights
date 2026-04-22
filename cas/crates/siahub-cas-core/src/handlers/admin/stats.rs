//! `GET /admin/stats` — aggregate usage_log rows for the session user.
//!
//! Backs CONSOLE-07 + CONSOLE-08. Pulls from Postgres (the durable source)
//! rather than Prometheus counters (ephemeral; reset on gateway restart —
//! RECEIVED.md §D recommendation).
//!
//! Invariant 3 (RECEIVED.md §C): canonical event literal for gateway
//! downloads is `'download'` (migration 0005 added the enum value). The
//! aggregation below uses that literal exclusively. Phase 2 writers emit
//! `xorb_upload` / `shard_upload` / `reconstruction`; the Phase 3 gateway
//! writes `download` with `cache_hit = Some(_)`. We count bytes_served on
//! the `download` rows only.

use axum::Json;
use axum::extract::State;
use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use crate::auth::AuthStateRef;
use crate::errors::AppError;
use crate::session::Session;

#[derive(Debug, Serialize)]
pub struct StatsResponse {
    pub total_bytes_stored: i64,
    pub total_bytes_served: i64,
    pub cache_hit_rate: f64,
    /// Number of distinct API keys the user has emitted events from. Real
    /// Sia host count surfaces separately via `/admin/stats/map` to keep
    /// this endpoint off the indexd hot path.
    pub provider_count: i64,
    pub per_key: Vec<PerKeyStats>,
    pub recent_activity: Vec<ActivityRow>,
}

#[derive(Debug, Serialize)]
pub struct PerKeyStats {
    pub key_id: Uuid,
    pub bytes_served: i64,
    pub bytes_stored: i64,
}

#[derive(Debug, Serialize)]
pub struct ActivityRow {
    pub ts: DateTime<Utc>,
    /// Hex-encoded xorb hash (byte-reversed-per-8-byte-group). For v1 the
    /// console only displays this as a prefix label — it never re-encodes
    /// it, so a plain hex view is acceptable here (the prefix search path
    /// uses the same encoding via `xorbs.hash_prefix_8`).
    pub hash: Option<String>,
    pub event: String,
    pub bytes: Option<i64>,
    pub cache_hit: Option<bool>,
}

/// `GET /admin/stats` — session-scoped aggregation. No admin flag required;
/// every user sees only their own rows.
pub async fn get_stats<S: AuthStateRef>(
    Session(user): Session,
    State(st): State<S>,
) -> Result<Json<StatsResponse>, AppError> {
    let pool = st.pool();

    // --- Top-level aggregate row. ---
    // The four sub-selects read:
    //   * bytes_stored — sum(size_bytes) of pinned xorbs owned by the user
    //   * bytes_served — sum(bytes) of event='download' rows joined to
    //                    keys owned by the user
    //   * cache_hit_rate — AVG over the downloads' cache_hit column
    //                      (coerced to 0.0/1.0; NULL rows excluded)
    //   * provider_count — distinct api_key_id seen in download events
    let row: (i64, i64, f64, i64) = sqlx::query_as(
        "WITH my_keys AS ( \
             SELECT id FROM api_keys WHERE user_id = $1 \
         ), \
         my_downloads AS ( \
             SELECT ul.bytes, ul.cache_hit, ul.api_key_id \
               FROM usage_log ul \
              WHERE ul.event = 'download' \
                AND ul.api_key_id IN (SELECT id FROM my_keys) \
         ) \
         SELECT \
             COALESCE((SELECT SUM(size_bytes) \
                         FROM xorbs \
                        WHERE owner_user_id = $1 \
                          AND pin_state = 'pinned'), 0)::bigint, \
             COALESCE((SELECT SUM(bytes) FROM my_downloads \
                        WHERE bytes IS NOT NULL), 0)::bigint, \
             COALESCE((SELECT AVG(CASE WHEN cache_hit THEN 1.0 ELSE 0.0 END) \
                         FROM my_downloads \
                        WHERE cache_hit IS NOT NULL), 0.0)::double precision, \
             COALESCE((SELECT COUNT(DISTINCT api_key_id) \
                         FROM my_downloads), 0)::bigint",
    )
    .bind(user.id)
    .fetch_one(pool)
    .await?;

    let (total_bytes_stored, total_bytes_served, cache_hit_rate, provider_count) = row;

    // --- Per-key breakdown. ---
    // Groups by api_key_id. bytes_served = SUM(bytes) over download rows;
    // bytes_stored = SUM(size_bytes) of xorbs owned by this key.
    let per_key_rows: Vec<(Uuid, i64, i64)> = sqlx::query_as(
        "SELECT ak.id, \
                COALESCE(served.bytes_served, 0)::bigint, \
                COALESCE(stored.bytes_stored, 0)::bigint \
           FROM api_keys ak \
           LEFT JOIN ( \
               SELECT api_key_id, SUM(bytes) AS bytes_served \
                 FROM usage_log \
                WHERE event = 'download' AND bytes IS NOT NULL \
                GROUP BY api_key_id \
           ) AS served ON served.api_key_id = ak.id \
           LEFT JOIN ( \
               SELECT owner_api_key_id, SUM(size_bytes) AS bytes_stored \
                 FROM xorbs \
                WHERE pin_state = 'pinned' \
                GROUP BY owner_api_key_id \
           ) AS stored ON stored.owner_api_key_id = ak.id \
          WHERE ak.user_id = $1 \
          ORDER BY served.bytes_served DESC NULLS LAST",
    )
    .bind(user.id)
    .fetch_all(pool)
    .await?;

    let per_key = per_key_rows
        .into_iter()
        .map(|(key_id, bytes_served, bytes_stored)| PerKeyStats {
            key_id,
            bytes_served,
            bytes_stored,
        })
        .collect();

    // --- Recent activity (20 most recent rows scoped to this user). ---
    type ActivityDbRow = (
        DateTime<Utc>,
        Option<Vec<u8>>,
        String,
        Option<i64>,
        Option<bool>,
    );
    let activity_rows: Vec<ActivityDbRow> = sqlx::query_as(
        "SELECT ul.occurred_at, \
                ul.xorb_hash, \
                ul.event::text, \
                ul.bytes, \
                ul.cache_hit \
           FROM usage_log ul \
           JOIN api_keys ak ON ul.api_key_id = ak.id \
          WHERE ak.user_id = $1 \
          ORDER BY ul.occurred_at DESC \
          LIMIT 20",
    )
    .bind(user.id)
    .fetch_all(pool)
    .await?;

    let recent_activity = activity_rows
        .into_iter()
        .map(|(ts, xorb_hash, event, bytes, cache_hit)| ActivityRow {
            ts,
            hash: xorb_hash.map(hex::encode_lowercase),
            event,
            bytes,
            cache_hit,
        })
        .collect();

    Ok(Json(StatsResponse {
        total_bytes_stored,
        total_bytes_served,
        cache_hit_rate,
        provider_count,
        per_key,
        recent_activity,
    }))
}

// Thin hex helper — we do not take a hex crate dep (workspace doesn't need
// it yet). Plain lowercase hex of BYTEA for the display-only activity row;
// NOT the merkle encoding (P1 byte-reversal is irrelevant for raw BYTEA
// column reads).
mod hex {
    pub fn encode_lowercase(bytes: Vec<u8>) -> String {
        let mut s = String::with_capacity(bytes.len() * 2);
        for b in bytes {
            s.push(char::from_digit((b >> 4) as u32, 16).unwrap());
            s.push(char::from_digit((b & 0x0f) as u32, 16).unwrap());
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::hex;

    #[test]
    fn hex_encode_is_lowercase() {
        assert_eq!(hex::encode_lowercase(vec![0xde, 0xad, 0xbe, 0xef]), "deadbeef");
        assert_eq!(hex::encode_lowercase(vec![0x00, 0xff]), "00ff");
    }

    #[test]
    fn hex_encode_empty_is_empty() {
        assert_eq!(hex::encode_lowercase(Vec::new()), "");
    }
}
