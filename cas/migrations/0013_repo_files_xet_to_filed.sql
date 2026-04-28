-- repo_files.xet_hash now points at reconstruction_files.file_id (xet's
-- per-file merkle hash) instead of xorbs.xorb_merkle_hash directly.
-- this matches the xet protocol: a file can span multiple xorbs, and a
-- xorb can hold pieces of multiple files. the file_id → xorb_hash + byte
-- range mapping is in reconstruction_terms.
ALTER TABLE repo_files DROP CONSTRAINT IF EXISTS repo_files_xet_hash_fkey;
-- no new fk: reconstruction_files may not exist yet at commit time for
-- legacy single-xorb-per-file uploads (the old "claim most recent xorb"
-- bridge stored xorb_merkle_hash here). on the new path, the resolve
-- handler joins through reconstruction_terms; absence simply yields 404.
