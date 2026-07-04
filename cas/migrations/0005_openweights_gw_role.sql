-- 0005_openweights_gw_role.sql — minimal-privilege GRANTs for the openweights_gw role.
-- Plan: 03-02-sia-postgres-adapter-PLAN.md · Phase: 03-openweights-gateway-core · Wave: 2
-- Owner: (openweights-gateway).
-- Scope split:
-- * ROLE CREATION happens at Postgres first-boot via
-- `ops/indexd-postgres-init.sql` (where the container entrypoint has env
-- var access and can interpolate OPENWEIGHTS_GW_PASSWORD via `\set`).
-- * PER-TABLE GRANTs live here because they depend on the `xorbs` +
-- `usage_log` tables existing — those are created by migrations 0002
-- and 0003, so this migration is sequenced after them.
-- Idempotent: `` against an already-granted role is a no-op in
-- Postgres. Replays against a bootstrapped cluster cost nothing.
-- CONTEXT §2 + KEY-DECISIONS : gateway writes 'download' rows to
-- `usage_log` directly (OQ-J resolved) and reads `xorb_merkle_hash →
-- sia_object_id` from `xorbs` filtered on `pin_state = 'pinned'`. Nothing
-- else. Widening this role requires a NEW migration — do NOT alter grants
-- in place (PITFALL forward-only).
-- If the role does not exist (recoverable dev path where init.sql never
-- ran because the volume was pre-bootstrapped), the DO block raises a
-- clear error instead of the GRANTs silently no-oping against a missing
-- role and leaving the gateway locked out.

DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'openweights_gw') THEN
        RAISE EXCEPTION
            'openweights_gw role does not exist; create it before applying 0005. '
            'Either rebuild the postgres volume (triggers ops/indexd-postgres-init.sql) '
            'or run: CREATE ROLE openweights_gw LOGIN PASSWORD ''<OPENWEIGHTS_GW_POSTGRES_PASSWORD>'';';
    END IF;
END
$$;

-- : add 'download' to the usage_event enum. Migration 0003 shipped the
-- enum with 'xorb_serve' / 'dedup_query' reserved for gateway use, but
-- KEY-DECISIONS overrides to converge on 'download' (matches
-- /04/08 stats surface). Forward-only ALTER; idempotent
-- via IF NOT EXISTS so migration replays are no-ops.
-- ALTER TYPE... ADD VALUE must run OUTSIDE a transaction on older
-- Postgres versions, but Postgres 12+ allows it in a transaction block.
-- sqlx::migrate! on Postgres 17 handles this transparently.
ALTER TYPE usage_event ADD VALUE IF NOT EXISTS 'download';

-- Schema access — required before any per-table resolves.
GRANT USAGE ON SCHEMA public TO openweights_gw;

-- Read-only on xorbs (the only table gateway reads).
-- Gateway query: SELECT sia_object_id, size_bytes FROM xorbs
-- WHERE xorb_merkle_hash = $1 AND pin_state = 'pinned';
GRANT SELECT ON TABLE xorbs TO openweights_gw;

-- Insert-only on usage_log (no UPDATE, no DELETE — CAS is append-only).
-- Gateway writes: event='download' + bytes + api_key_id + cache_hit + occurred_at.
GRANT INSERT ON TABLE usage_log TO openweights_gw;

-- Sequence USAGE lets INSERTs draw nextval from usage_log.id's BIGSERIAL
-- sequence. Omitting this surfaces as `permission denied for sequence
-- usage_log_id_seq` on every meter write.
GRANT USAGE ON SEQUENCE usage_log_id_seq TO openweights_gw;

-- NO grants on api_keys, users, sessions, oauth_state, shards,
-- reconstruction_files, reconstruction_terms. If the gateway ever needs
-- any of these, add an explicit in a NEW migration — do NOT widen
-- this role silently.
