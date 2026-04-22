-- 0006_sessions_touch.sql — Phase 4.01 (Plan 04-01) admin-routes amendment.
-- Plan: 04-01-phase-2.1-cas-admin-routes-PLAN.md · Phase: 04-siahub-console · Wave: W1
--
-- Purpose: add three small columns that Phase 4's console consumes but which
-- were out-of-scope for the Phase 2 schema freeze. All additive; no rename /
-- drop / destructive DDL (PITFALL P15 forward-only migrations).
--
-- 1. `sessions.last_seen_at` — rolling TTL bookkeeping (D-50 cookie spec).
--    `touch_session` in session.rs UPDATEs this column on every authenticated
--    request to power the 7-day rolling expiry.
-- 2. `api_keys.masked_prefix` — first 8 chars of the plaintext key stored at
--    creation time so GET /admin/keys can render a non-secret identifier
--    (D-45 guarantees plaintext appears exactly once, in POST response).
-- 3. `users.avatar_url` — GitHub avatar URL captured at OAuth callback and
--    `users.is_admin` — admin flag for /admin/xorbs + /admin/setup/status
--    gating (D-41 + Ambiguity 4 resolution). Admin bit is operator-managed
--    via direct SQL for v1 (anti-feature: no admin impersonation / team
--    management UI).
--
-- Idempotent: every ADD COLUMN uses IF NOT EXISTS; replay is a no-op.

ALTER TABLE sessions
    ADD COLUMN IF NOT EXISTS last_seen_at TIMESTAMPTZ NULL;

CREATE INDEX IF NOT EXISTS sessions_last_seen_idx
    ON sessions (last_seen_at)
    WHERE revoked_at IS NULL;

ALTER TABLE api_keys
    ADD COLUMN IF NOT EXISTS masked_prefix TEXT NULL;

ALTER TABLE users
    ADD COLUMN IF NOT EXISTS avatar_url TEXT NULL;

ALTER TABLE users
    ADD COLUMN IF NOT EXISTS is_admin BOOLEAN NOT NULL DEFAULT FALSE;
