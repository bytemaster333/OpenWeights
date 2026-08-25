#!/usr/bin/env bash
#CI-only test-key issuance helper.
# Mints a fresh OpenWeights API key by INSERTing directly into the `api_keys`
# table via `docker exec openweights-postgres psql`. Avoids modifying the frozen
# CAS Rust binary ( convention: amendments only via `N.X-*-PLAN.md`).
# SECURITY: This script is CI/dev-only. It bypasses the OAuth flow required
# by `POST /admin/keys`. NEVER run it against a production Postgres — the
# ci-test user it creates (`id=999999999`) would become an undetected
# permanent admin. The CI workflow tears down the stack immediately after.
# Matches the on-disk shape emitted by handlers/admin/keys.rs::create_key:
# - 32 random bytes → base64url-NO-padding (43-char plaintext)
# - SHA-256(plaintext) → stored as raw BYTEA in api_keys.key_hash
# - masked_prefix = first 8 chars + "..."
# - scopes = {<db-scope>} where --scope maps via the console contract:
# read → 'download'
# write → 'upload'
# admin → 'admin'
# Usage:
# bash scripts/issue-test-key.sh --scope write
# → prints the 43-char plaintext key to stdout (exit 0)
# Env:
# OPENWEIGHTS_POSTGRES_PASSWORD required; matches ops/.env seed
# COMPOSE_PG_SERVICE optional; default 'postgres' (container name
# resolved by `docker compose ps`). Override if
# running against a non-Compose Postgres.

set -euo pipefail

SCOPE="read"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --scope) SCOPE="$2"; shift 2 ;;
    -h|--help)
      sed -n '1,40p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
  esac
done

case "$SCOPE" in
  read)      DB_SCOPES_SQL="ARRAY['download'::api_key_scope]" ;;
  write)     DB_SCOPES_SQL="ARRAY['upload'::api_key_scope]" ;;
  # readwrite = both, so one key completes a full upload+download round-trip.
  readwrite) DB_SCOPES_SQL="ARRAY['download','upload']::api_key_scope[]" ;;
  admin)     DB_SCOPES_SQL="ARRAY['admin'::api_key_scope]" ;;
  *) echo "invalid --scope '$SCOPE' (expected read|write|readwrite|admin)" >&2; exit 2 ;;
esac

: "${OPENWEIGHTS_POSTGRES_PASSWORD:?OPENWEIGHTS_POSTGRES_PASSWORD must be exported (matches .env)}"
PG_SERVICE="${COMPOSE_PG_SERVICE:-openweights-postgres}"

# Generate 32 random bytes → base64url-NO-pad. Uses `openssl rand` for
# cross-platform deterministic output. python3 fallback for minimal images.
if command -v openssl >/dev/null 2>&1; then
  PLAINTEXT=$(openssl rand 32 | base64 | tr '+/' '-_' | tr -d '=\n')
elif command -v python3 >/dev/null 2>&1; then
  PLAINTEXT=$(python3 -c "import os, base64; print(base64.urlsafe_b64encode(os.urandom(32)).decode().rstrip('='))")
else
  echo "need openssl or python3 to generate random key material" >&2
  exit 2
fi

# First 8 chars + "..." — same shape as the create_key handler emits.
MASKED="${PLAINTEXT:0:8}..."

# SHA-256 of the plaintext bytes → 64-char lowercase hex → BYTEA via
# Postgres's decode('<hex>', 'hex').
if command -v openssl >/dev/null 2>&1; then
  KEY_HASH_HEX=$(printf '%s' "$PLAINTEXT" | openssl dgst -sha256 -hex | awk '{print $2}')
elif command -v python3 >/dev/null 2>&1; then
  KEY_HASH_HEX=$(python3 -c "import hashlib,sys; print(hashlib.sha256(sys.argv[1].encode()).hexdigest())" "$PLAINTEXT")
else
  echo "need openssl or python3 to compute SHA-256" >&2
  exit 2
fi

# Fixed CI test user ID — chosen outside the plausible GitHub user.id range
# (GitHub user IDs are allocated sequentially; 999999999 is well beyond any
# real account that would share this Postgres). Idempotent upsert so repeat
# CI runs reuse the same row.
CI_USER_ID=999999999
# Must match the owner namespace the round-trip harness uploads under
# (tests/hf-roundtrip/standalone-roundtrip.sh -> openweights-e2e/...): the HF
# read path resolves `/api/models/{owner}/{repo}` by joining repos to
# users.github_login, so a mismatch here makes every download 404.
CI_USER_LOGIN="openweights-e2e"

# psql exec shim. COMPOSE Postgres container runs as user `postgres`; the
# `openweights` database was created by indexd-postgres-init.sql at first boot.
psql_exec() {
  docker exec -e PGPASSWORD="$OPENWEIGHTS_POSTGRES_PASSWORD" "$PG_SERVICE" \
    psql -v ON_ERROR_STOP=1 -U openweights -d openweights -tAc "$1"
}

# Upsert the CI test user (ON CONFLICT DO NOTHING — second call is a no-op).
psql_exec "INSERT INTO users (id, github_login, email) VALUES ($CI_USER_ID, '$CI_USER_LOGIN', NULL) ON CONFLICT (id) DO UPDATE SET github_login = EXCLUDED.github_login;" >/dev/null

# Insert the api_keys row. key_hash is BYTEA — feed it as \x-prefixed hex.
psql_exec "INSERT INTO api_keys (user_id, key_hash, scopes, label, masked_prefix) VALUES ($CI_USER_ID, decode('$KEY_HASH_HEX', 'hex'), $DB_SCOPES_SQL, 'ci-test-$(date +%s)', '$MASKED');" >/dev/null

# Plaintext to stdout — harness captures via $(cmd).
printf '%s\n' "$PLAINTEXT"
