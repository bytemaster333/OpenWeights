-- 0004_xorbs_nullable_sia_id.sql — forward-only relax of xorbs.sia_object_id.
-- Plan: 02-04-xorb-upload-handler-PLAN.md · Phase: 02-siahub-cas-core · Wave: 3
--
-- Background: Plan 02-02 landed `xorbs.sia_object_id BYTEA NOT NULL` (matching
-- ARCHITECTURE.md's sketch). Plan 02-04's handler flow needs to INSERT the
-- xorb row BEFORE the Sia upload completes so the ON CONFLICT DO NOTHING
-- dedup claim is atomic — at that moment `sia_object_id` is not yet known.
--
-- Options surveyed in Plan 02-04 (see its PLAN §Task 3 "Schema reconciliation"):
--   (a) forward-only ALTER on a separate migration file — chosen, this file;
--   (b) amend 0002 in-place — rejected because 02-02 has already been
--       applied by the W2 executor to the dev/CI Postgres instances.
--
-- PITFALL P15 (forward-only migrations): no DROP / rename / destructive DDL.
-- After this file runs, new INSERTs may omit sia_object_id; existing rows
-- keep their already-populated values. Reconciler (Plan 02-09) backfills via
-- UPDATE once Sia confirms the pin.
--
-- Idempotent: IF EXISTS + already-nullable check — replay is a no-op.
DO $$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM information_schema.columns
        WHERE table_schema = 'public'
          AND table_name   = 'xorbs'
          AND column_name  = 'sia_object_id'
          AND is_nullable  = 'NO'
    ) THEN
        ALTER TABLE xorbs ALTER COLUMN sia_object_id DROP NOT NULL;
    END IF;
END $$;
