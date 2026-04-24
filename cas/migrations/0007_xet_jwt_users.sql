-- amendment — siahub-hf-proxy integration.
-- Adds an alternate identity column so that CAS can upsert a user row
-- keyed on Hugging Face's internal user ID (as found in HF's Xet JWT
-- payload), independent of the GitHub-OAuth-keyed identity used by the
-- console.
-- Why a new column instead of a new table:
-- * Downstream tables (xorbs, shards, usage_log) already FK to users.id.
-- Re-pointing them to a union table would be a multi-migration mess.
-- * `github_login` retains its NOT NULL invariant by stuffing the HF
-- user ID behind the `hf:` prefix — no real GitHub login can start
-- with that.
-- `users.id` derivation for HF-auth'd rows:
-- * JWT payload `userId` is a 24-hex string (Mongo ObjectId shape).
-- * We deterministically hash it into a positive i64 via
-- `(SHA-256(hf_user_id)[0..8] as i64) & 0x7FFF_FFFF_FFFF_FFFF`.
-- * Collision risk at our scale (v1 demo, <1e5 users) is ~2^-32
-- acceptable. A collision would cause two distinct HF users to share
-- the same SiaHub user_id row; a follow-up v1.1 migration can split
-- if it ever happens.

ALTER TABLE users
    ADD COLUMN IF NOT EXISTS hf_user_id TEXT NULL;

-- Unique only across non-NULL values so the existing GitHub users
-- (hf_user_id IS NULL) don't collide with each other.
CREATE UNIQUE INDEX IF NOT EXISTS users_hf_user_id_key
    ON users (hf_user_id)
    WHERE hf_user_id IS NOT NULL;

-- Fast lookup path for the hot-ish JWT auth codepath.
CREATE INDEX IF NOT EXISTS users_hf_user_id_idx
    ON users (hf_user_id)
    WHERE hf_user_id IS NOT NULL AND revoked_at IS NULL;
