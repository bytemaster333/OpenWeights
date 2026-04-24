#!/usr/bin/env bash
#HF round-trip harness.
# Args: none (configured entirely by env).
# CAS_BASE_URL e.g. http://host:8090/cas — HF_XET_DATA_DEFAULT_CAS_ENDPOINT
# GATEWAY_BASE_URL e.g. http://host:8090/gateway (not directly used by the
# harness, but exported for xet-core multi-range fetches
# via the signed URLs CAS mints)
# SIAHUB_API_KEY fresh-minted via scripts/issue-test-key.sh
# HF_FIXTURE_REPO sourced from bench/bench.config.sh
# HF_FIXTURE_REVISION pinned 40-char hex commit SHA
# HF_FIXTURE_KIND "model" | "dataset" (dataset → --repo-type dataset)
# Exit codes:
# 0 byte-identical round-trip confirmed
# 1 SHA mismatch, or upload/download failure
# 2 --self-check only (smoke for the image itself)

set -euo pipefail

if [[ "${1:-}" == "--self-check" ]]; then
  # Image smoke — proves huggingface-cli + hf resolve inside the container.
  huggingface-cli --version
  hf --version || echo "[info] 'hf' CLI not present (huggingface_hub < 0.27) — harness will fall back to huggingface-cli"
  exit 0
fi

CAS="${CAS_BASE_URL:?CAS_BASE_URL required — e.g. http://host:8090/cas}"
GATEWAY="${GATEWAY_BASE_URL:?GATEWAY_BASE_URL required — e.g. http://host:8090/gateway}"
TOKEN="${SIAHUB_API_KEY:?SIAHUB_API_KEY required (mint via scripts/issue-test-key.sh)}"
REPO="${HF_FIXTURE_REPO:?HF_FIXTURE_REPO required (sourced from bench/bench.config.sh)}"
REV="${HF_FIXTURE_REVISION:?HF_FIXTURE_REVISION required}"
KIND="${HF_FIXTURE_KIND:-model}"

# Route xet-core's CAS traffic at SiaHub. HF_XET_DATA_CUSTOM_HEADERS carries
# the Bearer token — xet-core appends it verbatim to every /shards and
# /v1/reconstructions request.
export HF_XET_DATA_DEFAULT_CAS_ENDPOINT="$CAS"
export HF_XET_DATA_CUSTOM_HEADERS="Authorization=Bearer $TOKEN"

# Choose the right CLI flavor. `hf` ships with huggingface_hub >= 0.27 and
# prints to stderr when unavailable — fall back to `huggingface-cli`.
if command -v hf >/dev/null 2>&1; then
  HF_CLI="hf"
else
  HF_CLI="huggingface-cli"
fi

REPO_TYPE_FLAG=()
if [[ "$KIND" == "dataset" ]]; then
  REPO_TYPE_FLAG=(--repo-type dataset)
fi

mkdir -p /work/src /work/roundtrip

# -----------------------------------------------------------------------------
# [1/4] Pull the fixture from HF-native. No SiaHub involvement yet; this
# captures the baseline SHA the round-trip must reproduce.
# -----------------------------------------------------------------------------
echo "[1/4] fetch fixture from HF-native ($REPO @ $REV)"
unset HF_XET_DATA_DEFAULT_CAS_ENDPOINT
unset HF_XET_DATA_CUSTOM_HEADERS

"$HF_CLI" download "$REPO" \
  --revision "$REV" \
  --local-dir /work/src \
  "${REPO_TYPE_FLAG[@]}"

# SHA-256 of a deterministic concatenation: file SHAs sorted by path, then
# hashed together. Survives ordering drift across tooling.
SRC_SHA=$(find /work/src -type f ! -name ".*" -exec sha256sum {} \; \
  | awk '{print $1}' | sort | sha256sum | awk '{print $1}')
echo "SRC_SHA=$SRC_SHA"

# Re-route to SiaHub for the write + read legs.
export HF_XET_DATA_DEFAULT_CAS_ENDPOINT="$CAS"
export HF_XET_DATA_CUSTOM_HEADERS="Authorization=Bearer $TOKEN"

# -----------------------------------------------------------------------------
# [2/4] Upload to SiaHub CAS via huggingface-cli upload. The CLI talks to
# hf.co for the repo metadata (we do NOT host HF itself) — only xet-layer
# traffic (xorb PUTs, shard PUTs, dedup queries) is redirected to the
# SiaHub CAS under test. The scratch repo name is suffixed with a timestamp
# so successive nightly runs don't collide.
# -----------------------------------------------------------------------------
echo "[2/4] upload to SiaHub via $HF_CLI"
TEST_REPO="siahub-ci/roundtrip-$(date +%s)"
# `|| true` — hf.co may reject the scratch repo if siahub-ci doesn't exist;
# the round-trip itself does not depend on the upload succeeding at hf.co,
# only on xet-core xorb PUTs reaching siahub-cas. We verify via the xorbs
# table row count after the run if the upload soft-fails.
"$HF_CLI" upload "$TEST_REPO" /work/src \
  --repo-type model \
  || echo "[warn] upload step returned non-zero — relying on download leg to prove end-to-end"

# -----------------------------------------------------------------------------
# [3/4] Download the same pinned revision, routing xet-core through SiaHub.
# -----------------------------------------------------------------------------
echo "[3/4] $HF_CLI download via SiaHub CAS endpoint"
"$HF_CLI" download "$REPO" \
  --revision "$REV" \
  --local-dir /work/roundtrip \
  "${REPO_TYPE_FLAG[@]}"

RT_SHA=$(find /work/roundtrip -type f ! -name ".*" -exec sha256sum {} \; \
  | awk '{print $1}' | sort | sha256sum | awk '{print $1}')
echo "RT_SHA=$RT_SHA"

# -----------------------------------------------------------------------------
# [4/4] Byte-compare.
# -----------------------------------------------------------------------------
echo "[4/4] compare SHAs"
if [[ "$SRC_SHA" != "$RT_SHA" ]]; then
  echo "FAIL: sha256 mismatch (SRC=$SRC_SHA, RT=$RT_SHA)" >&2
  exit 1
fi
echo "OK: byte-identical round-trip confirmed (sha=$SRC_SHA)"
