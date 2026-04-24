-- 0010_repo_downloads.sql — per-repo daily download counters.
-- The gateway writes `usage_log (event='xorb_serve')` rows, but
-- the HF-API-compat download path (migration 0008) serves bytes directly
-- from CAS — bypassing the gateway, so no audit trail accumulates. This
-- migration adds a small denormalized counter table that the
-- `lfs_download` + `xet_file_serve` handlers bump on every successful
-- byte-serve; Console reads feed off this table instead of aggregating
-- `usage_log` across four-hop joins at query time.
-- Rows are keyed on `(repo_id, day)` so the trend query degenerates to a
-- simple index scan over the partial range. The repo_id FK is
-- `ON DELETE CASCADE` so revoking / deleting a repo cleans its counters
-- in the same transaction — no orphaned rows possible.

CREATE TABLE IF NOT EXISTS repo_downloads (
    repo_id    BIGINT NOT NULL REFERENCES repos(id) ON DELETE CASCADE,
    day        DATE   NOT NULL,
    count      BIGINT NOT NULL DEFAULT 0,
    bytes      BIGINT NOT NULL DEFAULT 0,
    PRIMARY KEY (repo_id, day)
);

-- Global "what happened today across the deployment" scans use this
-- index; per-repo trend queries hit the PK directly.
CREATE INDEX IF NOT EXISTS repo_downloads_day_idx
    ON repo_downloads (day DESC);
