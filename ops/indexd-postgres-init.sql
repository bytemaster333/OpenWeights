-- ops/indexd-postgres-init.sql
-- Runs ONCE on first Postgres container boot (via /docker-entrypoint-initdb.d/).
-- Creates the two databases SiaHub needs:
--   indexd   — used by the indexd daemon itself (required per RESEARCH §9)
--   siahub   — used by siahub-cas / siahub-gateway (reserved for Phase 2+)
--
-- Passwords come from the postgres container's environment variables:
--   INDEXD_PASSWORD  — maps to .env INDEXD_POSTGRES_PASSWORD
--   SIAHUB_PASSWORD  — maps to .env SIAHUB_POSTGRES_PASSWORD
--
-- Docker entrypoint sources /docker-entrypoint-initdb.d/*.sql as the superuser
-- postgres (authed via POSTGRES_PASSWORD env var).

\set indexd_password `echo "$INDEXD_PASSWORD"`
\set siahub_password `echo "$SIAHUB_PASSWORD"`

-- indexd daemon DB
CREATE ROLE indexd LOGIN PASSWORD :'indexd_password';
CREATE DATABASE indexd OWNER indexd ENCODING 'UTF8' TEMPLATE template0;
GRANT ALL PRIVILEGES ON DATABASE indexd TO indexd;

-- siahub-cas / gateway DB (Phase 2+ populates schema)
CREATE ROLE siahub LOGIN PASSWORD :'siahub_password';
CREATE DATABASE siahub OWNER siahub ENCODING 'UTF8' TEMPLATE template0;
GRANT ALL PRIVILEGES ON DATABASE siahub TO siahub;

-- Sanity: list the created databases on first boot for visible operator confirmation.
\echo 'SiaHub Postgres init complete. Databases created: indexd, siahub'
\l
