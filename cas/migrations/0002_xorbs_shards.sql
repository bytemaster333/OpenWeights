-- 0002_xorbs_shards.sql — core CAS data model.
-- ARCHITECTURE.md §"Postgres schema sketch" + RESEARCH deltas
-- (hash_prefix_8 generated col + pin_state tracking on both xorbs and shards).
-- Plan: 02-02-schema-migrations-PLAN.md · Phase: 02-siahub-cas-core · Wave: 2
-- Hashes are BYTEA (PITFALL ). xet-core's merkle-hash encoding is
-- byte-reversed-per-8-byte-group hex — if anyone ever needs a text view,
-- decode via `xet_core_structures::merklehash::MerkleHash`, NEVER a plain
-- `encode(hash, 'hex')`. The hash_prefix_8 generated column below is the ONE
-- hex-encoded view allowed, and only because prefix search is
-- already operating on the display form of the hash.

-- === pin-state enum — shared by xorbs and shards ===
-- States:
-- 'uploading' — bytes not yet persisted to Sia
-- 'pinning' — uploaded, pin_object pending/retrying
-- 'pinned' — pin_object succeeded; only state served by reconstruction
-- 'orphaned' — 5+ failed pin attempts; ops must manually re-drive or wipe
-- Reconciler tick (60 s) advances 'uploading'→'pinning'→'pinned'
-- via sdk.upload retry + sdk.pin_object. See RESEARCH §4.3 + PITFALL .
DO $$ BEGIN
    CREATE TYPE xorb_pin_state AS ENUM (
        'uploading',
        'pinning',
        'pinned',
        'orphaned'
    );
EXCEPTION WHEN duplicate_object THEN NULL; END $$;

-- === xorbs ===
-- Primary key: xorb_merkle_hash as raw 32-byte BYTEA (PITFALL — NEVER hex).
-- sia_object_id is the Hash256 returned by sdk.upload (RESEARCH §4.1);
-- content-addressed so retries against a previously-uploaded body yield the
-- same id ( reconciler relies on this).
-- pin_state + pin_attempts + last_pin_attempt_at implement / .
-- hash_prefix_8 ( item 3) is a GENERATED STORED column
-- prefix search uses it via a plain B-tree index.
CREATE TABLE IF NOT EXISTS xorbs (
    xorb_merkle_hash     BYTEA PRIMARY KEY,
    sia_object_id        BYTEA NOT NULL,
    size_bytes           BIGINT NOT NULL,
    owner_user_id        BIGINT NOT NULL REFERENCES users(id),
    owner_api_key_id     UUID   NOT NULL REFERENCES api_keys(id),
    uploaded_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    pin_state            xorb_pin_state NOT NULL DEFAULT 'pinning',
    pin_attempts         INT  NOT NULL DEFAULT 0,
    last_pin_attempt_at  TIMESTAMPTZ NULL,
    hash_prefix_8        TEXT GENERATED ALWAYS AS (
        substring(encode(xorb_merkle_hash, 'hex') FROM 1 FOR 8)
    ) STORED
);
CREATE INDEX IF NOT EXISTS xorbs_owner_idx
    ON xorbs (owner_user_id, uploaded_at DESC);
CREATE INDEX IF NOT EXISTS xorbs_key_idx
    ON xorbs (owner_api_key_id, uploaded_at DESC);
-- item 3 — console prefix search. Unconditional index since
-- hash_prefix_8 is generated STORED and therefore non-null for every row.
CREATE INDEX IF NOT EXISTS xorbs_prefix_idx
    ON xorbs (hash_prefix_8);
-- Reconciler hot-path: sweep rows still making progress or waiting to retry.
-- Partial index keeps the index tiny — the overwhelming majority of rows are
-- 'pinned' and therefore excluded.
CREATE INDEX IF NOT EXISTS xorbs_pin_sweep_idx
    ON xorbs (pin_state, last_pin_attempt_at)
    WHERE pin_state IN ('uploading', 'pinning');

-- === shards ===
-- option C: INSERT shard + reconstruction_* rows in one tx; THEN call
-- sdk.upload + sdk.pin_object; THEN UPDATE sia_object_id + pin_state='pinned'.
-- Hence sia_object_id is NULL while pin_state IN ('uploading','pinning').
-- Shard pin-state mirror of xorbs applies / to shards too.
CREATE TABLE IF NOT EXISTS shards (
    shard_hash           BYTEA PRIMARY KEY,
    sia_object_id        BYTEA NULL,
    size_bytes           BIGINT NOT NULL,
    owner_user_id        BIGINT NOT NULL REFERENCES users(id),
    owner_api_key_id     UUID   NOT NULL REFERENCES api_keys(id),
    uploaded_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    pin_state            xorb_pin_state NOT NULL DEFAULT 'pinning',
    pin_attempts         INT  NOT NULL DEFAULT 0,
    last_pin_attempt_at  TIMESTAMPTZ NULL
);
CREATE INDEX IF NOT EXISTS shards_pin_sweep_idx
    ON shards (pin_state, last_pin_attempt_at)
    WHERE pin_state IN ('uploading', 'pinning');

-- === reconstruction_files ===
-- file_id = 32-byte merkle hash of the reconstructed file (BYTEA, NOT hex).
-- One-to-one with the shard that registered it (shard_hash FK).
CREATE TABLE IF NOT EXISTS reconstruction_files (
    file_id       BYTEA PRIMARY KEY,
    shard_hash    BYTEA NOT NULL REFERENCES shards(shard_hash),
    total_size    BIGINT NOT NULL,
    registered_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- === reconstruction_terms ===
-- IMPORTANT comment-invariant (PITFALL — every conversion site
-- annotated in handler code too):
-- xorb_start..xorb_end — chunk-index range, END-EXCLUSIVE.
-- xorb_byte_start..xorb_byte_end — pre-computed byte offsets in the xorb,
-- END-EXCLUSIVE. Per RESEARCH §2.5 these
-- are computed once at shard-INSERT time
-- from the chunk-length table, avoiding a
-- Sia footer re-fetch on every
-- reconstruction query.
-- unpacked_start..unpacked_end — END-EXCLUSIVE byte offsets in the
-- reconstructed file.
-- The single `- 1` conversion to end-inclusive byte range lives in
-- coalesce_terms_by_xorb; the schema stays end-exclusive.
CREATE TABLE IF NOT EXISTS reconstruction_terms (
    file_id          BYTEA NOT NULL REFERENCES reconstruction_files(file_id),
    term_index       INT   NOT NULL,
    xorb_hash        BYTEA NOT NULL REFERENCES xorbs(xorb_merkle_hash),
    xorb_start       BIGINT NOT NULL,
    xorb_end         BIGINT NOT NULL,
    xorb_byte_start  BIGINT NOT NULL,
    xorb_byte_end    BIGINT NOT NULL,
    unpacked_start   BIGINT NOT NULL,
    unpacked_end     BIGINT NOT NULL,
    PRIMARY KEY (file_id, term_index)
);
CREATE INDEX IF NOT EXISTS reconstruction_terms_xorb_idx
    ON reconstruction_terms (xorb_hash);
