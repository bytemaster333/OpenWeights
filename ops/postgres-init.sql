-- ops/postgres-init.sql
-- Runs ONCE on first Postgres container boot (via /docker-entrypoint-initdb.d/).
-- Creates the openweights database + roles. OpenWeights does not bundle indexd,
-- so there is no indexd database here — the indexer is external
-- (OPENWEIGHTS_INDEXER_URL, e.g. https://sia.storage or a self-hosted indexd).
-- Passwords come from the postgres container's environment variables:
--   OPENWEIGHTS_PASSWORD    — maps to .env OPENWEIGHTS_POSTGRES_PASSWORD
--   OPENWEIGHTS_GW_PASSWORD — maps to .env OPENWEIGHTS_GW_POSTGRES_PASSWORD
-- Docker entrypoint sources /docker-entrypoint-initdb.d/*.sql as the superuser
-- postgres (authed via POSTGRES_PASSWORD env var).

\set openweights_password `echo "$OPENWEIGHTS_PASSWORD"`
\set openweights_gw_password `echo "$OPENWEIGHTS_GW_PASSWORD"`

-- openweights-cas / gateway DB (CAS migration runner populates the schema)
CREATE ROLE openweights LOGIN PASSWORD :'openweights_password';
CREATE DATABASE openweights OWNER openweights ENCODING 'UTF8' TEMPLATE template0;
GRANT ALL PRIVILEGES ON DATABASE openweights TO openweights;

-- openweights-gateway: dedicated minimal-privilege login role.
-- Role CREATION lives here (first-boot only) because the sqlx-embedded CAS
-- migration runner does not inject env vars into SQL. Per-table GRANTs for
-- this role live in cas/migrations/0005_openweights_gw_role.sql (applied later,
-- once the `xorbs` + `usage_log` tables exist).
-- If OPENWEIGHTS_GW_PASSWORD is unset at init time the CREATE ROLE is skipped;
-- the operator recovers by running:
--   docker compose exec -e PGPASSWORD=$POSTGRES_SUPERUSER_PASSWORD postgres \\
--     psql -U postgres -c "CREATE ROLE openweights_gw LOGIN PASSWORD '...';"
-- This is the volume-rehydration path, not the happy path.
CREATE ROLE openweights_gw LOGIN PASSWORD :'openweights_gw_password';
GRANT CONNECT ON DATABASE openweights TO openweights_gw;

-- Sanity: list the created databases on first boot for visible operator confirmation.
\echo 'OpenWeights Postgres init complete. Databases + roles: openweights, openweights_gw.'
\l
