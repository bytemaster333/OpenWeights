#!/usr/bin/env bash
# Plan 05-02 — P12 range-integrity regression (explicit test).
#
# Cheaper-than-full-round-trip assertion that Caddy does NOT corrupt Range
# responses. Issues (a) single-range and (b) multi-range GETs through the
# Caddy reverse proxy at the gateway and checks:
#
#   1. HTTP 206 Partial Content status.
#   2. `Content-Range: bytes <s>-<e>/<total>` on single-range responses.
#   3. `Content-Type: multipart/byteranges; boundary=<b>` on multi-range
#      responses. A concatenated body silently corrupts xet-core V2 downloads
#      (notes.md gotcha #3) — THE load-bearing assertion of this script.
#   4. The boundary string appears at least twice in the body (one part-open
#      marker + one close marker).
#
# Invocation from CI:
#   GATEWAY_VIA_CADDY_URL=http://localhost:8090/gateway \
#   CAS_VIA_CADDY_URL=http://localhost:8090/cas \
#   SIAHUB_API_KEY=<fresh key> \
#   bash tests/hf-roundtrip/verify-range-integrity.sh
#
# The script mints its own signed URL via CAS /v1/reconstructions given an
# uploaded xorb hash (harness pre-populates one via a small fixture upload).
# If SIGNED_URL_PATH is provided directly, the minting step is skipped.
#
# Exit codes:
#   0  single + multi range through Caddy passed all checks
#   1  any assertion failed
#   2  pre-condition missing (no signed URL + no xorb to mint one from)

set -euo pipefail

GATEWAY="${GATEWAY_VIA_CADDY_URL:?must point at Caddy path-prefix route, e.g. http://localhost:8090/gateway}"

# Two modes:
#   (a) direct: caller passes SIGNED_URL_PATH (everything after the gateway base).
#   (b) mint: caller passes CAS_VIA_CADDY_URL + SIAHUB_API_KEY + XORB_HASH, we
#       fetch a signed URL from /v1/reconstructions/<hash> and peel the gateway
#       path out of the returned URL.
URL=""
if [[ -n "${SIGNED_URL_PATH:-}" ]]; then
  URL="${GATEWAY}/${SIGNED_URL_PATH#/}"
elif [[ -n "${XORB_HASH:-}" && -n "${CAS_VIA_CADDY_URL:-}" && -n "${SIAHUB_API_KEY:-}" ]]; then
  echo "[prep] minting signed URL via CAS reconstruction for xorb $XORB_HASH"
  # v1 reconstruction query returns a list of segment URLs; pick the first
  # part's URL and rewrite its origin to the Caddy path-prefix so we exercise
  # the reverse proxy.
  RESP=$(curl -fsS -H "Authorization: Bearer ${SIAHUB_API_KEY}" \
    "${CAS_VIA_CADDY_URL}/v1/reconstructions/${XORB_HASH}")
  # Extract the first .terms[].url field. Fallback: jq if present, else grep.
  if command -v jq >/dev/null 2>&1; then
    URL=$(echo "$RESP" | jq -r '.terms[0].url')
  else
    URL=$(echo "$RESP" | grep -oE '"url":"[^"]+"' | head -1 | sed 's/^"url":"//' | sed 's/"$//')
  fi
  if [[ -z "$URL" ]]; then
    echo "FAIL: could not parse signed URL from /v1/reconstructions response" >&2
    echo "raw response: $RESP" >&2
    exit 2
  fi
else
  echo "pre-condition missing: either SIGNED_URL_PATH or (XORB_HASH + CAS_VIA_CADDY_URL + SIAHUB_API_KEY)" >&2
  exit 2
fi

echo "[prep] target URL: $URL"

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

# -----------------------------------------------------------------------------
# [1/2] Single-range: Range: bytes=0-15 → expect 206 + Content-Range: bytes 0-15/*
# -----------------------------------------------------------------------------
echo "[1/2] single-range"
HDRS_FILE="$TMP/single.hdr"
BODY_FILE="$TMP/single.bin"
curl -fsS --http2 -D "$HDRS_FILE" -o "$BODY_FILE" \
  -H 'Range: bytes=0-15' "$URL"

if ! grep -qE 'HTTP/[12](\.[0-9]+)? 206' "$HDRS_FILE"; then
  echo "FAIL: expected HTTP 206 Partial Content on single-range" >&2
  cat "$HDRS_FILE" >&2
  exit 1
fi

if ! grep -qiE '^content-range: bytes 0-15/' "$HDRS_FILE"; then
  echo "FAIL: expected 'Content-Range: bytes 0-15/<total>' on single-range" >&2
  cat "$HDRS_FILE" >&2
  exit 1
fi

SINGLE_LEN=$(wc -c < "$BODY_FILE" | tr -d ' ')
if [[ "$SINGLE_LEN" -ne 16 ]]; then
  echo "FAIL: single-range body expected 16 bytes, got $SINGLE_LEN" >&2
  exit 1
fi
echo "  ok: 206 + Content-Range + 16 bytes"

# -----------------------------------------------------------------------------
# [2/2] Multi-range: Range: bytes=0-7,16-23 → expect 206 + multipart/byteranges
# -----------------------------------------------------------------------------
echo "[2/2] multi-range (P12 load-bearing assertion)"
HDRS_FILE="$TMP/multi.hdr"
BODY_FILE="$TMP/multi.bin"
curl -fsS --http2 -D "$HDRS_FILE" -o "$BODY_FILE" \
  -H 'Range: bytes=0-7,16-23' "$URL"

if ! grep -qE 'HTTP/[12](\.[0-9]+)? 206' "$HDRS_FILE"; then
  echo "FAIL: expected HTTP 206 Partial Content on multi-range" >&2
  cat "$HDRS_FILE" >&2
  exit 1
fi

# P12 PRIMARY ASSERTION — a concatenated body silently corrupts xet-core.
if ! grep -qiE '^content-type: multipart/byteranges; boundary=' "$HDRS_FILE"; then
  echo "FAIL (P12): expected 'Content-Type: multipart/byteranges; boundary=<b>' on multi-range" >&2
  echo "            Concatenated body would silently corrupt xet-core V2 downloads." >&2
  cat "$HDRS_FILE" >&2
  exit 1
fi

# Extract the boundary string. Case-insensitive header; boundary token is
# defined by RFC 2046 §5.1.1 (bcharsnospace).
BOUNDARY=$(grep -ioE 'boundary=[^;[:space:]]+' "$HDRS_FILE" | head -1 | cut -d= -f2 | tr -d '"')
if [[ -z "$BOUNDARY" ]]; then
  echo "FAIL: could not parse multipart boundary from Content-Type" >&2
  exit 1
fi
echo "  boundary: $BOUNDARY"

# Boundary must appear ≥ 2 times in the body — one per part-open marker
# (`--<boundary>`) plus the close marker (`--<boundary>--`). In practice
# with 2 parts this is 3 occurrences. We check ≥ 2 to tolerate weird edge
# cases in boundary string selection (e.g., if Caddy ever re-emits a single
# boundary instance, the test still catches "no boundary at all").
BOUNDARY_COUNT=$(grep -cF -- "$BOUNDARY" "$BODY_FILE" || true)
if [[ "$BOUNDARY_COUNT" -lt 2 ]]; then
  echo "FAIL: multipart boundary '$BOUNDARY' appeared only $BOUNDARY_COUNT times in body (expected ≥ 2)" >&2
  exit 1
fi
echo "  ok: boundary appeared $BOUNDARY_COUNT times in body"

echo "OK: single + multi range integrity through Caddy verified (P12 mitigated)"
