-- 0003_usage_log_oauth.sql — metering + session + OAuth state tables.
-- Plan: 02-02-schema-migrations-PLAN.md · Phase: 02-siahub-cas-core · Wave: 2
--
-- D-19 item 1: 'reconstruction' added to usage_event (Phase 4 CONSOLE-03..08
--              reconstruction counters depend on it).
-- D-19 item 2: usage_log_event_cache_idx partial index (Phase 4 cache-hit
--              aggregation path).
-- D-17:        synchronous single-row INSERT per event in the handler tx
--              path. No batcher, no channel — upgrade path documented in
--              CONTEXT if v1 scale demands it.

-- === usage_event enum ===
-- Writers:
--   'xorb_upload'    — Plan 02-04 xorb handler (Phase 2).
--   'shard_upload'   — Plan 02-05 shard handler (Phase 2).
--   'reconstruction' — Plan 02-06 V1 reconstruction handler (Phase 2).
--   'xorb_serve'     — Phase 3 gateway signed-URL hit path.
--   'dedup_query'    — reserved for future non-stub of PROTO-04
--                      (D-16 currently stubs to 404).
-- Enum order is fixed; Phase 3's usage-log writer presumes this exact set
-- even though concrete queries don't care about ordinal position.
DO $$ BEGIN
    CREATE TYPE usage_event AS ENUM (
        'xorb_upload',
        'shard_upload',
        'reconstruction',
        'xorb_serve',
        'dedup_query'
    );
EXCEPTION WHEN duplicate_object THEN NULL; END $$;

-- === usage_log ===
-- Append-only (CAS is append-only per notes.md §What NOT to Do).
-- user_id + api_key_id both nullable so the Phase 3 gateway can write
-- cache_hit=true 'xorb_serve' rows without necessarily resolving the key
-- (e.g. for anonymous-cache-hit accounting, OQ-J).
CREATE TABLE IF NOT EXISTS usage_log (
    id           BIGSERIAL PRIMARY KEY,
    event        usage_event NOT NULL,
    api_key_id   UUID   NULL REFERENCES api_keys(id),
    user_id      BIGINT NULL REFERENCES users(id),
    xorb_hash    BYTEA  NULL,
    shard_hash   BYTEA  NULL,
    file_id      BYTEA  NULL,
    bytes        BIGINT NULL,
    cache_hit    BOOLEAN NULL,   -- populated by Phase 3 gateway writes (D-17 / OQ-J)
    occurred_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS usage_log_key_time_idx
    ON usage_log (api_key_id, occurred_at DESC);
CREATE INDEX IF NOT EXISTS usage_log_user_time_idx
    ON usage_log (user_id,    occurred_at DESC);
-- D-19 item 2 — Phase 4 stats queries need a cheap (event, cache_hit)
-- aggregation. Partial-index `WHERE cache_hit IS NOT NULL` excludes the
-- synchronously-written Phase 2 rows (xorb_upload / shard_upload /
-- reconstruction) that never set cache_hit, keeping the index small.
CREATE INDEX IF NOT EXISTS usage_log_event_cache_idx
    ON usage_log (event, cache_hit)
    WHERE cache_hit IS NOT NULL;

-- === sessions ===
-- Phase 4 /admin OAuth cookies land here. session_id is a random UUID v4
-- held in a HttpOnly cookie; lookups are by PK so no extra index needed.
-- Partial index on (user_id) WHERE revoked_at IS NULL supports "list my
-- active sessions" in the console.
CREATE TABLE IF NOT EXISTS sessions (
    session_id   UUID PRIMARY KEY,
    user_id      BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at   TIMESTAMPTZ NOT NULL,
    revoked_at   TIMESTAMPTZ NULL
);
CREATE INDEX IF NOT EXISTS sessions_user_active_idx
    ON sessions (user_id)
    WHERE revoked_at IS NULL;

-- === oauth_state ===
-- Per-OAuth-start opaque random token; consumed exactly once on callback.
-- Expired/consumed rows are swept periodically (Plan 02-09 reconciler scope).
CREATE TABLE IF NOT EXISTS oauth_state (
    state        TEXT PRIMARY KEY,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    consumed_at  TIMESTAMPTZ NULL,
    expires_at   TIMESTAMPTZ NOT NULL
);
