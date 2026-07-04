//! api_keys table queries.
//! TODO(phase-2-plan-02-03): implement `fetch_active_key_by_hash(hash: &[u8;32])`
//! returning the active row (revoked_at IS NULL) for auth middleware.
//! Also `touch_last_used(id)` — can be fire-and-forget write.
//! Schema reference: cas/migrations/0001_initial.sql.
//! Remember: key_hash is raw 32-byte BYTEA, NOT hex — compare against
//! `Sha256::digest(plaintext).into::<[u8;32]>` directly.
