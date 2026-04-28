-- per-file sha256 from xet shard's FileMetadataExt; matches the OID hf_hub
-- sends in commit lfsFile entries. lookup by sha256 is the bridge from
-- commit to reconstruction.
ALTER TABLE reconstruction_files
    ADD COLUMN IF NOT EXISTS sha256 BYTEA;

CREATE INDEX IF NOT EXISTS reconstruction_files_sha256_idx
    ON reconstruction_files (sha256)
    WHERE sha256 IS NOT NULL;
