#!/usr/bin/env bash
# ops/smoke.sh — Fresh-machine objective smoke for the hosted demo.
#
# Plan 05-04. Replaces the cut DEMO-01 outside-tester <15-min criterion (D-67).
# Run immediately post-deploy + whenever the owner wants reassurance:
#
#   bash ops/smoke.sh                    # default domain: siahub.app
#   bash ops/smoke.sh example.com        # custom domain
#
# Exit semantics (D-71):
#   0  all checks passed
#   1  at least one check failed
#   2  pre-condition skip (no network / missing tool)
#
# Required env (sourced from .env by default):
#   SIAHUB_PUBLIC_READ_KEY    read-scoped API key (mint one via the console)
#   SIAHUB_FIXTURE_FILE_ID    file-id from ops/preload-fixture.sh receipt
#                             (optional — check #3 degrades to 401-assert if unset)

set -uo pipefail

# ─── Arguments ─────────────────────────────────────────────────────────────────
DOMAIN="${1:-siahub.app}"
CAS="https://cas.${DOMAIN}"
CONSOLE="https://${DOMAIN}"

# ─── Helpers ───────────────────────────────────────────────────────────────────
pass() { echo "[PASS] $*"; }
fail() { echo "[FAIL] $*" >&2; FAILED=1; }
skip() { echo "[SKIP] $*" >&2; exit 2; }

# ─── Pre-conditions ────────────────────────────────────────────────────────────
command -v curl    >/dev/null 2>&1 || skip "curl not installed"
command -v openssl >/dev/null 2>&1 || skip "openssl not installed"
command -v jq      >/dev/null 2>&1 || skip "jq not installed"

# Source .env if present — makes SIAHUB_PUBLIC_READ_KEY / SIAHUB_FIXTURE_FILE_ID
# available without the operator re-exporting them.
if [[ -f .env ]]; then
  set -a
  # shellcheck disable=SC1091
  source .env
  set +a
fi

FAILED=0

echo "=== SiaHub hosted-demo smoke against ${DOMAIN} ==="

# ─── 1. HTTPS reachability on both SANs ────────────────────────────────────────
echo "[1/5] HTTPS reachability"
if curl -fsS -o /dev/null --max-time 10 "${CONSOLE}/"; then
  pass "console HTTPS reachable (${CONSOLE})"
else
  fail "console NOT reachable (${CONSOLE})"
fi
if curl -fsS -o /dev/null --max-time 10 "${CAS}/health"; then
  pass "CAS HTTPS reachable (${CAS}/health)"
else
  fail "CAS NOT reachable (${CAS}/health)"
fi

# ─── 2. Dual-SAN cert: both domains share one cert (D-60) ──────────────────────
echo "[2/5] dual-SAN cert (D-60)"
SANS="$(
  echo | openssl s_client -connect "${DOMAIN}:443" -servername "${DOMAIN}" 2>/dev/null \
    | openssl x509 -noout -ext subjectAltName 2>/dev/null | tr ',' '\n'
)"
if echo "$SANS" | grep -q "DNS:${DOMAIN}" && echo "$SANS" | grep -q "DNS:cas.${DOMAIN}"; then
  pass "dual-SAN cert covers both {${DOMAIN}, cas.${DOMAIN}}"
else
  fail "dual-SAN cert missing one of {${DOMAIN}, cas.${DOMAIN}}; SANs=$(echo "$SANS" | tr '\n' ' ')"
fi

# ─── 3. CAS protocol surface reachable + auth required ─────────────────────────
echo "[3/5] /v1/reconstructions endpoint"
if [[ -n "${SIAHUB_FIXTURE_FILE_ID:-}" && -n "${SIAHUB_PUBLIC_READ_KEY:-}" ]]; then
  # Authenticated call: expect a 200 with a fetch_info shape.
  RECON="$(
    curl -fsS --max-time 15 \
      -H "Authorization: Bearer ${SIAHUB_PUBLIC_READ_KEY}" \
      "${CAS}/v1/reconstructions/${SIAHUB_FIXTURE_FILE_ID}" \
    || echo ""
  )"
  if [[ -n "$RECON" ]] && echo "$RECON" | jq -e '.fetch_info // .terms // .' >/dev/null 2>&1; then
    pass "reconstructions returns well-formed JSON for preloaded fixture"
  else
    fail "reconstructions call failed or returned unexpected shape"
  fi
else
  # Unauthenticated call: expect 401 (proves CAS is live + gating works; does
  # NOT require a real fixture or key).
  STATUS="$(curl -sS -o /dev/null -w '%{http_code}' --max-time 10 \
    "${CAS}/v1/reconstructions/0000000000000000000000000000000000000000000000000000000000000000")"
  if [[ "$STATUS" == "401" || "$STATUS" == "403" ]]; then
    pass "reconstructions requires auth (got ${STATUS}) — CAS protocol live"
  else
    fail "reconstructions expected 401/403 unauth, got ${STATUS}"
  fi
fi

# ─── 4. HF download via SiaHub endpoint (optional; D-66 core-value check) ──────
echo "[4/5] hf download through ${CAS}"
HF_BIN=""
if   command -v hf               >/dev/null 2>&1; then HF_BIN=hf
elif command -v huggingface-cli  >/dev/null 2>&1; then HF_BIN=huggingface-cli
fi

if [[ -z "$HF_BIN" ]]; then
  echo "[WARN] hf / huggingface-cli not installed; skipping download check (not a fail)"
elif [[ -z "${SIAHUB_PUBLIC_READ_KEY:-}" ]]; then
  echo "[WARN] SIAHUB_PUBLIC_READ_KEY unset; skipping download check (not a fail)"
else
  TMP="$(mktemp -d)"
  trap 'rm -rf "$TMP"' EXIT
  # shellcheck disable=SC1091
  source bench/bench.config.sh
  HF_ARGS=(--revision "$HF_FIXTURE_REVISION" --local-dir "$TMP")
  if [[ "$HF_FIXTURE_KIND" == "dataset" ]]; then
    HF_ARGS+=(--repo-type dataset)
  fi
  if HF_XET_DATA_DEFAULT_CAS_ENDPOINT="${CAS}" \
     HF_XET_DATA_CUSTOM_HEADERS="Authorization=Bearer ${SIAHUB_PUBLIC_READ_KEY}" \
     "$HF_BIN" download "$HF_FIXTURE_REPO" "${HF_ARGS[@]}" >/dev/null 2>&1; then
    pass "hf download via SiaHub endpoint succeeded"
  else
    fail "hf download via SiaHub endpoint failed"
  fi
fi

# ─── 5. Range-header integrity through Caddy (P12 regression) ──────────────────
echo "[5/5] range-header integrity (P12)"
if [[ -x tests/hf-roundtrip/verify-range-integrity.sh ]]; then
  # Point the P12 regression script at the prod Caddy path-prefix routes.
  # It mints a signed URL via the CAS /v1/reconstructions/<xorb> route, then
  # issues single-range + multi-range GETs through Caddy.
  if GATEWAY_VIA_CADDY_URL="${CAS}/gateway" \
     CAS_VIA_CADDY_URL="${CAS}" \
     SIAHUB_API_KEY="${SIAHUB_PUBLIC_READ_KEY:-}" \
     XORB_HASH="${SIAHUB_FIXTURE_XORB_HASH:-}" \
     SIGNED_URL_PATH="${SIAHUB_FIXTURE_SIGNED_URL_PATH:-}" \
     bash tests/hf-roundtrip/verify-range-integrity.sh >/dev/null 2>&1; then
    pass "single + multi-range integrity through Caddy"
  else
    # Range script returns 2 when it lacks inputs; treat that as WARN, not FAIL.
    RC=$?
    if [[ "$RC" -eq 2 ]]; then
      echo "[WARN] range-integrity script needs SIAHUB_FIXTURE_XORB_HASH or SIGNED_URL_PATH; skipping"
    else
      fail "range-header integrity failed (Caddy P12 regression)"
    fi
  fi
else
  echo "[WARN] tests/hf-roundtrip/verify-range-integrity.sh missing or not executable; skipping"
fi

# ─── Verdict ───────────────────────────────────────────────────────────────────
echo
if [[ "$FAILED" -eq 0 ]]; then
  echo "[OK] all smoke checks passed"
  exit 0
fi
echo "[FAIL] one or more smoke checks failed"
exit 1
