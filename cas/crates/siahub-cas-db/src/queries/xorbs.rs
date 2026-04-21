//! xorbs table queries — insert_pending / set_pin_state / get_sia_object_id / exists_pinned.
//!
//! The state machine (CONTEXT D-15):
//!
//! ```text
//!     insert_pending(INSERT pin_state='pinning' ON CONFLICT DO NOTHING)
//!       ├─ Ok(true)   → caller performs sdk.upload_and_pin, then
//!       │               set_pin_state('pinned', Some(real_id))
//!       │               (on Sia failure: leave 'pinning'; reconciler retries)
//!       └─ Ok(false)  → dedup path, caller returns {was_inserted: false}
//! ```
//!
//! Plan 02-09 reconciler sweeps rows stuck in 'uploading' or 'pinning' older
//! than 5 minutes; after 5 attempts it transitions to 'orphaned'.
//!
//! NOTE on typed enums: we use runtime-checked `sqlx::query_as` / `sqlx::query`
//! with string bind ("pinning", "pinned", ...) because binding a `sqlx::Type`
//! custom enum via `query!` requires the macro to observe the Postgres schema
//! at compile time — that is handled by the `cargo sqlx prepare --workspace`
//! step. In this crate's current state the `.sqlx/` cache is empty; we keep
//! the runtime-checked path so `cargo build` succeeds with `SQLX_OFFLINE=true`
//! without needing a live DB. Plan 02-09 may upgrade these to compile-checked
//! `query!` calls once the offline cache is regenerated.

use sqlx::PgPool;
use uuid::Uuid;

use crate::types::XorbPinState;

/// Attempt to claim the xorb hash. Returns `Ok(true)` on fresh INSERT,
/// `Ok(false)` when the hash already exists (dedup — caller returns
/// `{was_inserted: false}` with no Sia I/O).
///
/// Initial `pin_state` is schema-default `'pinning'` (Plan 02-02, D-15
/// deviation). `sia_object_id` is left NULL until the upload+pin succeeds;
/// Migration 0004 relaxed the NOT NULL constraint specifically to make this
/// possible (see that file's comment).
pub async fn insert_pending(
    pool: &PgPool,
    xorb_hash: &[u8; 32],
    size_bytes: i64,
    owner_user_id: i64,
    owner_api_key_id: Uuid,
) -> Result<bool, sqlx::Error> {
    // RETURNING xorb_merkle_hash → Some(_) on insert, None on ON CONFLICT.
    let row: Option<(Vec<u8>,)> = sqlx::query_as(
        "INSERT INTO xorbs (xorb_merkle_hash, sia_object_id, size_bytes, owner_user_id, owner_api_key_id) \
         VALUES ($1, NULL, $2, $3, $4) \
         ON CONFLICT (xorb_merkle_hash) DO NOTHING \
         RETURNING xorb_merkle_hash",
    )
    .bind(&xorb_hash[..])
    .bind(size_bytes)
    .bind(owner_user_id)
    .bind(owner_api_key_id)
    .fetch_optional(pool)
    .await?;

    Ok(row.is_some())
}

/// Update `pin_state`. When `sia_object_id` is provided, it is written (or
/// overwrites NULL); when `None`, the existing column value is retained via
/// `COALESCE`. Also bumps `pin_attempts` and stamps `last_pin_attempt_at`.
///
/// The pin_state column is a Postgres enum; we pass it as its lowercase text
/// representation (matching `#[sqlx(type_name = "xorb_pin_state", rename_all
/// = "lowercase")]`).
pub async fn set_pin_state(
    pool: &PgPool,
    xorb_hash: &[u8; 32],
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
        "UPDATE xorbs \
         SET pin_state = $1::xorb_pin_state, \
             sia_object_id = COALESCE($2, sia_object_id), \
             pin_attempts = pin_attempts + 1, \
             last_pin_attempt_at = NOW() \
         WHERE xorb_merkle_hash = $3",
    )
    .bind(state_text)
    .bind(sia_object_id)
    .bind(&xorb_hash[..])
    .execute(pool)
    .await?;

    Ok(())
}

/// Return the `sia_object_id` for the xorb **only if** it is in the `pinned`
/// state. Any other state (uploading / pinning / orphaned) returns `None` so
/// reconstruction queries naturally 404 for not-yet-durable xorbs (D-15).
pub async fn get_sia_object_id(
    pool: &PgPool,
    xorb_hash: &[u8; 32],
) -> Result<Option<Vec<u8>>, sqlx::Error> {
    let row: Option<(Option<Vec<u8>>,)> = sqlx::query_as(
        "SELECT sia_object_id FROM xorbs \
         WHERE xorb_merkle_hash = $1 AND pin_state = 'pinned'",
    )
    .bind(&xorb_hash[..])
    .fetch_optional(pool)
    .await?;

    // Outer Option = row present?  Inner Option = column NULL?  Collapse both:
    // a `pinned` row SHOULD never have NULL sia_object_id, but be defensive.
    Ok(row.and_then(|(id,)| id))
}

/// `true` iff the xorb has been successfully pinned to Sia. Used by Plan
/// 02-05 shard cross-check (P18) and by Plan 02-06 reconstruction filter.
pub async fn exists_pinned(
    pool: &PgPool,
    xorb_hash: &[u8; 32],
) -> Result<bool, sqlx::Error> {
    let row: Option<(i32,)> = sqlx::query_as(
        "SELECT 1 FROM xorbs WHERE xorb_merkle_hash = $1 AND pin_state = 'pinned'",
    )
    .bind(&xorb_hash[..])
    .fetch_optional(pool)
    .await?;

    Ok(row.is_some())
}
