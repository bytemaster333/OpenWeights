-- 0009_xorb_bodies.sql — inline cache of xorb body bytes for V1 download.
-- hf_xet upload bodies are chunks + headers (no XorbObject footer in the
-- 1.5.0-dev1 wire format). To serve downloads before 's gateway
-- lands a Sia range-fetch path, we stash the raw upload body alongside
-- the `xorbs` row and decompress chunks at serve time.
-- Tradeoff: doubles local storage vs. the Sia-only story. Acceptable for
-- the grant demo and for `pin_state != 'pinned'` rows we'd need a local
-- copy anyway. will retire this table in favor of
-- `/v1/reconstructions/{file_id}` → gateway signed URL → Sia range fetch.

CREATE TABLE IF NOT EXISTS xorb_bodies (
    xorb_hash   BYTEA PRIMARY KEY REFERENCES xorbs(xorb_merkle_hash) ON DELETE CASCADE,
    content     BYTEA NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
