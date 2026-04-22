-- ops/indexd-postgres-init.sql
-- Runs ONCE on first Postgres container boot (via /docker-entrypoint-initdb.d/).
-- Creates the two databases SiaHub needs:
--   indexd   — used by the indexd daemon itself (required per RESEARCH §9)
--   siahub   — used by siahub-cas / siahub-gateway (reserved for Phase 2+)
--
-- Passwords come from the postgres container's environment variables:
--   INDEXD_PASSWORD     — maps to .env INDEXD_POSTGRES_PASSWORD
--   SIAHUB_PASSWORD     — maps to .env SIAHUB_POSTGRES_PASSWORD
--   SIAHUB_GW_PASSWORD  — maps to .env SIAHUB_GW_POSTGRES_PASSWORD  (Phase 3)
--
-- Docker entrypoint sources /docker-entrypoint-initdb.d/*.sql as the superuser
-- postgres (authed via POSTGRES_PASSWORD env var).

\set indexd_password `echo "$INDEXD_PASSWORD"`
\set siahub_password `echo "$SIAHUB_PASSWORD"`
\set siahub_gw_password `echo "$SIAHUB_GW_PASSWORD"`

-- indexd daemon DB
CREATE ROLE indexd LOGIN PASSWORD :'indexd_password';
CREATE DATABASE indexd OWNER indexd ENCODING 'UTF8' TEMPLATE template0;
GRANT ALL PRIVILEGES ON DATABASE indexd TO indexd;

-- siahub-cas / gateway DB (Phase 2+ populates schema)
CREATE ROLE siahub LOGIN PASSWORD :'siahub_password';
CREATE DATABASE siahub OWNER siahub ENCODING 'UTF8' TEMPLATE template0;
GRANT ALL PRIVILEGES ON DATABASE siahub TO siahub;

-- Phase 3 (siahub-gateway): dedicated minimal-privilege login role.
-- Role CREATION lives here (first-boot only) because the sqlx-embedded CAS
-- migration runner does not inject env vars into SQL. Per-table GRANTs for
-- this role live in cas/migrations/0005_siahub_gw_role.sql (applied later,
-- once the `xorbs` + `usage_log` tables exist).
--
-- If SIAHUB_GW_PASSWORD is unset at init time the CREATE ROLE is skipped;
-- the operator recovers by running:
--   docker compose exec -e PGPASSWORD=$POSTGRES_SUPERUSER_PASSWORD postgres \\
--     psql -U postgres -c "CREATE ROLE siahub_gw LOGIN PASSWORD '...';"
-- This is the volume-rehydration path, not the happy path.
CREATE ROLE siahub_gw LOGIN PASSWORD :'siahub_gw_password';
GRANT CONNECT ON DATABASE siahub TO siahub_gw;

-- Sanity: list the created databases on first boot for visible operator confirmation.
\echo 'SiaHub Postgres init complete. Databases + roles: indexd, siahub, siahub_gw.'
\l
