#!/usr/bin/env bash
# Plan 05-01 Task 3 — wait for the 6-service Compose stack to become healthy.
#
# Polls Docker Compose healthcheck state (NOT raw HTTP) so services whose
# health probes are internal (pg_isready, redis-cli PING, siahub-readiness)
# are respected exactly as docker-compose.yml specifies. Per-service timeout
# budgets reflect expected boot cost:
#
#   postgres          30s  (cold init runs indexd-postgres-init.sql)
#   redis             30s
#   indexd           600s  (first-boot Zen consensus header sync)
#   siahub-cas       120s  (sqlx migrations + Axum bind)
#   siahub-gateway   120s  (cache-dir init + Postgres attach)
#   siahub-console    60s  (static nginx — fast)
#
# Exit 0 when every service reports `healthy`; exit 1 on any timeout.

set -euo pipefail

COMPOSE_FILE="${COMPOSE_FILE:-ops/docker-compose.yml}"
CI_OVERLAY="${CI_OVERLAY:-ops/docker-compose.ci.yml}"
ENV_FILE="${ENV_FILE:-.env}"

compose() {
  docker compose -f "$COMPOSE_FILE" -f "$CI_OVERLAY" --env-file "$ENV_FILE" "$@"
}

# Usage: wait_healthy <service_name> <timeout_seconds>
wait_healthy() {
  local svc="$1" timeout="$2" start status
  start=$(date +%s)
  while true; do
    # `docker compose ps --format json` emits one JSON object per line in
    # v2; parse the Health field for the named service. Falls back to
    # `State` when Health is empty (services without a healthcheck — none
    # in our stack today, but keep the path robust).
    status="$(compose ps --format json 2>/dev/null \
      | awk -v svc="$svc" 'BEGIN{RS="\n"} $0 ~ "\"Service\":\""svc"\"" {print}' \
      | python3 -c 'import json,sys
for line in sys.stdin:
    line = line.strip()
    if not line: continue
    try:
        obj = json.loads(line)
    except Exception:
        continue
    h = obj.get("Health") or obj.get("State") or ""
    print(h)
    break' 2>/dev/null || echo "")"

    if [ "$status" = "healthy" ]; then
      echo "[OK] $svc healthy"
      return 0
    fi

    if (( $(date +%s) - start > timeout )); then
      echo "[FAIL] $svc did not reach healthy within ${timeout}s (last status: '${status}')" >&2
      compose ps >&2 || true
      return 1
    fi

    sleep 3
  done
}

echo "Waiting for SiaHub compose stack to become healthy..."

# Order reflects compose dependency graph. Foundations first, then services.
wait_healthy postgres         30
wait_healthy redis            30
wait_healthy indexd          600
wait_healthy siahub-cas      120
wait_healthy siahub-gateway  120
wait_healthy siahub-console   60

echo "[OK] all services healthy"
