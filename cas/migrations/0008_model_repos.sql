-- 0008_model_repos.sql — SiaHub as a standalone model hub.
-- V1 extension that lets SiaHub be the HF-compatible hub itself, not just a
-- Xet-protocol CAS. Adds the smallest schema that makes `hf upload` /
-- `hf download` work against `hf.siahub.app` without any huggingface.co
-- round-trip:
-- repos — (owner_user_id, name) namespace a la HF's {user}/{repo}
-- repo_refs — branch/tag pointers (main → commit_id)
-- repo_commits — commit metadata (author, message, parent) — Git-ish
-- repo_files — per-commit file manifest (path → xet_hash | lfs_oid)
-- Invariants:
-- * `{owner_user_id, name}` is unique — matches HF's `{user}/{repo}` flat
-- namespace. `name` follows HF's charset: [a-zA-Z0-9_.-]{1,96}.
-- * `visibility = 'public' | 'private'` — V1 lists only 'public' in the
-- catalog; 'private' is reserved for v1.1 and is honored at read-time.
-- * `repo_refs.ref_name` is a branch/tag name. V1 writes only 'main'. The
-- table is not restricted so v1.1 can add more refs without a migration.
-- * `repo_files.xet_hash` FKs to xorbs.xorb_merkle_hash when the file is
-- Xet-stored. For small sidecar files (README.md, config.json) that
-- hf_xet routes through classic LFS, `lfs_oid` holds the SHA-256 and
-- `xet_hash` is NULL. Exactly-one-of the two MUST be non-NULL.
-- * Commits are immutable; "push to main" writes a new row and updates
-- repo_refs.commit_id. No history truncation — rewrite is append-only.
-- Rationale: HF.co does NOT expose an external-backend API for
-- its Xet repos. To make byte-on-Sia flows work end-to-end with hf CLI, we
-- must be the HF API server too. This migration is the minimum pointer layer
-- that lets commit validation happen against our own xorbs table instead of
-- HF's internal Xet index.

-- === repo visibility enum ===
-- 'unlisted' is reserved for v1.1 catalog-opt-out without making the
-- repo private. V1 treats 'unlisted' the same as 'public' for any explicit
-- URL access.
DO $$ BEGIN
    CREATE TYPE repo_visibility AS ENUM ('public', 'private', 'unlisted');
EXCEPTION WHEN duplicate_object THEN NULL; END $$;

-- === repos ===
-- HF's namespace shape is {owner}/{name}. Owner is always a user row
-- (users.id BIGINT). console GitHub OAuth populates the `users` row;
-- 's xet JWT path uses hf_user_id → synthetic user. Either kind of
-- owner can create a repo.
-- `name` charset + length matches HF's UI constraint (docs: "1-96 chars,
-- letters/numbers/./-/_"). Enforced at the API boundary, not as a CHECK, so
-- we can relax or tighten without a migration.
CREATE TABLE IF NOT EXISTS repos (
    id               BIGSERIAL PRIMARY KEY,
    owner_user_id    BIGINT NOT NULL REFERENCES users(id),
    name             TEXT   NOT NULL,
    visibility       repo_visibility NOT NULL DEFAULT 'public',
    description      TEXT   NULL,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (owner_user_id, name)
);
CREATE INDEX IF NOT EXISTS repos_owner_idx ON repos (owner_user_id, updated_at DESC);
-- Public catalog lookup: list by visibility, sorted by recency.
CREATE INDEX IF NOT EXISTS repos_public_idx ON repos (updated_at DESC)
    WHERE visibility = 'public';

-- === repo_commits ===
-- Commits form a chain via parent_commit_id; HEAD-of-main lives in repo_refs.
-- V1 won't walk history, but we store it so log/diff can be added without
-- a second migration. Merkle-style content hash not computed; the BIGSERIAL
-- id is sufficient for the pointer-layer use case.
CREATE TABLE IF NOT EXISTS repo_commits (
    id                 BIGSERIAL PRIMARY KEY,
    repo_id            BIGINT NOT NULL REFERENCES repos(id),
    parent_commit_id   BIGINT NULL REFERENCES repo_commits(id),
    author_user_id     BIGINT NOT NULL REFERENCES users(id),
    author_api_key_id  UUID   NOT NULL REFERENCES api_keys(id),
    message            TEXT   NOT NULL,
    committed_at       TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS repo_commits_repo_idx ON repo_commits (repo_id, committed_at DESC);

-- === repo_refs ===
-- Branch/tag pointers. V1 writes only 'main'. `commit_id` is the HEAD of that
-- ref. Updated atomically on commit acceptance.
CREATE TABLE IF NOT EXISTS repo_refs (
    repo_id      BIGINT NOT NULL REFERENCES repos(id),
    ref_name     TEXT   NOT NULL,
    commit_id    BIGINT NOT NULL REFERENCES repo_commits(id),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (repo_id, ref_name)
);

-- === repo_files ===
-- Per-commit file manifest. Two storage backends:
-- * Xet: `xet_hash` references xorbs.xorb_merkle_hash. Bytes live in
-- the CAS / Sia; reconstruction uses existing shards/reconstruction_*
-- tables to resolve file_id → xorb ranges.
-- * LFS: `lfs_oid` holds the classic SHA-256. Used by hf_hub for small
-- sidecar files (README.md, config.json) that bypass Xet.
-- Exactly one of (xet_hash, lfs_oid) is non-NULL; the CHECK enforces it.
CREATE TABLE IF NOT EXISTS repo_files (
    commit_id    BIGINT NOT NULL REFERENCES repo_commits(id),
    path         TEXT   NOT NULL,
    size_bytes   BIGINT NOT NULL,
    -- Xet-backed: FK to xorbs so dangling pointers are impossible at
    -- commit time. Note a single file may span multiple xorbs in xet-core's
    -- shard model; for v1 we store the root xet_hash and rely on the
    -- reconstruction_* tables to resolve ranges.
    xet_hash     BYTEA  NULL REFERENCES xorbs(xorb_merkle_hash),
    -- LFS-backed: SHA-256 of the file bytes. Stored but not FK'd because
    -- small files live on disk/local cache, not in the xorbs table.
    lfs_oid      BYTEA  NULL,
    PRIMARY KEY (commit_id, path),
    CHECK ((xet_hash IS NOT NULL) <> (lfs_oid IS NOT NULL))
);
CREATE INDEX IF NOT EXISTS repo_files_xet_idx ON repo_files (xet_hash)
    WHERE xet_hash IS NOT NULL;

-- === small-file LFS storage (V1 inline) ===
-- Sidecar files (README.md, config.json, .gitattributes) under ~5MB stored
-- inline as BYTEA. Keeps the pointer-layer self-contained; no need to stand
-- up a separate object store for text files. Larger LFS objects are not
-- supported in V1 — authors upload them via the Xet path instead.
CREATE TABLE IF NOT EXISTS lfs_objects (
    oid          BYTEA PRIMARY KEY,
    size_bytes   BIGINT NOT NULL,
    content      BYTEA  NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CHECK (octet_length(content) <= 5 * 1024 * 1024)
);
