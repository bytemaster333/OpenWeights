-- 0001_initial.sql — CAS base schema.
-- Authored per RESEARCH §6.1 + ARCHITECTURE.md §"Postgres schema sketch".
-- Plan: 02-02-schema-migrations-PLAN.md · Phase: 02-openweights-cas-core · Wave: 2
-- Idempotency invariant (T-02- / PITFALL ): every CREATE in this file
-- must be safe to re-run. sqlx's _sqlx_migrations table already tracks the
-- checksum, but testcontainers / CI scratch volumes re-execute so we belt +
-- suspenders it with IF NOT EXISTS on tables/indexes and a
-- DO $$...EXCEPTION WHEN duplicate_object$$ wrapper around CREATE TYPE
-- (Postgres 17 does not accept IF NOT EXISTS on CREATE TYPE).

-- === extensions ===
CREATE EXTENSION IF NOT EXISTS pgcrypto;   -- gen_random_uuid

-- === enums ===
DO $$ BEGIN
    CREATE TYPE api_key_scope AS ENUM ('upload', 'download', 'admin');
EXCEPTION WHEN duplicate_object THEN NULL; END $$;

-- === users ===
-- PK is GitHub numeric user.id (BIGINT; PITFALL — email may be null or
-- the @users.noreply.github.com masked form). NEVER key users by email.
CREATE TABLE IF NOT EXISTS users (
    id           BIGINT PRIMARY KEY,
    github_login TEXT   NOT NULL UNIQUE,
    email        TEXT   NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    revoked_at   TIMESTAMPTZ NULL
);
CREATE INDEX IF NOT EXISTS users_login_idx
    ON users (github_login)
    WHERE revoked_at IS NULL;

-- === api_keys ===
-- key_hash is raw SHA-256(plaintext), 32 BYTEA bytes — NEVER stored as hex.
-- : handlers compute `Sha256::digest(plaintext).into::<[u8;32]>` and
-- compare against key_hash directly. Any future contributor attempting hex
-- storage will hit a BYTEA type mismatch at INSERT time.
-- scopes is an array of enum so one key can carry e.g. {upload, download}.
CREATE TABLE IF NOT EXISTS api_keys (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id       BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    key_hash      BYTEA  NOT NULL UNIQUE,
    scopes        api_key_scope[] NOT NULL,
    label         TEXT   NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    revoked_at    TIMESTAMPTZ NULL,
    last_used_at  TIMESTAMPTZ NULL
);
CREATE INDEX IF NOT EXISTS api_keys_user_idx
    ON api_keys (user_id)
    WHERE revoked_at IS NULL;
CREATE INDEX IF NOT EXISTS api_keys_hash_idx
    ON api_keys (key_hash)
    WHERE revoked_at IS NULL;
