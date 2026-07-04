#!/usr/bin/env bash
# ops/preload-fixture.sh — Seed the hosted demo with the pinned fixture model.
#. One-shot: run ONCE after the first `make deploy` and
# after minting a write-scoped API key in the console. Idempotent on re-run
# huggingface-cli dedups by Merkle hash, so re-uploading the same fixture is
# a ~3-second no-op.
# Exit codes:
# 0 fixture uploaded (or already present)
# 1 any step failed
# 2 pre-condition missing (env vars / CLI / bench config)
# Pre-conditions:
# - `make deploy` completed; stack healthy; Caddy has Let's Encrypt cert.
# - A write-scoped API key minted via the /keys page of https://openweights.app.
# - `hf` (>= 0.27) or `huggingface-cli` on PATH.
# - bench/bench.config.sh source-of-truth checked in.
# Invocation:
# OPENWEIGHTS_CAS_URL=https://cas.openweights.app \
# OPENWEIGHTS_WRITE_KEY=hf_sia_... \
# bash ops/preload-fixture.sh

set -euo pipefail

# ─── Pre-conditions ────────────────────────────────────────────────────────────
: "${OPENWEIGHTS_CAS_URL:?must be set, e.g. https://cas.openweights.app}"
: "${OPENWEIGHTS_WRITE_KEY:?write-scoped API key minted via the console}"

if [[ ! -f bench/bench.config.sh ]]; then
  echo "pre-condition missing: bench/bench.config.sh" >&2
  exit 2
fi

HF_BIN=""
if   command -v hf               >/dev/null 2>&1; then HF_BIN=hf
elif command -v huggingface-cli  >/dev/null 2>&1; then HF_BIN=huggingface-cli
else
  echo "pre-condition missing: neither 'hf' nor 'huggingface-cli' on PATH" >&2
  echo "install with: pip install 'huggingface_hub[cli,hf_xet]>=0.25,<1.0'" >&2
  exit 2
fi

# shellcheck disable=SC1091
source bench/bench.config.sh

# ─── Route xet-core uploads through the hosted OpenWeights CAS ──────────────────────
export HF_XET_DATA_DEFAULT_CAS_ENDPOINT="${OPENWEIGHTS_CAS_URL}"
export HF_XET_DATA_CUSTOM_HEADERS="Authorization=Bearer ${OPENWEIGHTS_WRITE_KEY}"

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

echo "=== Preload fixture to hosted demo ==="
echo "  CAS:      ${OPENWEIGHTS_CAS_URL}"
echo "  fixture:  ${HF_FIXTURE_REPO} @ ${HF_FIXTURE_REVISION}"
echo "  kind:     ${HF_FIXTURE_KIND}"
echo

# ─── [1/3] Fetch fixture from HF (xet-native download; bypasses OpenWeights) ───────
echo "[1/3] Fetching fixture from Hugging Face..."
DL_ARGS=(--revision "$HF_FIXTURE_REVISION" --local-dir "$TMP")
if [[ "$HF_FIXTURE_KIND" == "dataset" ]]; then
  DL_ARGS+=(--repo-type dataset)
fi
# Temporarily unset OpenWeights routing so the initial fetch comes straight from HF.
(
  unset HF_XET_DATA_DEFAULT_CAS_ENDPOINT HF_XET_DATA_CUSTOM_HEADERS
  "$HF_BIN" download "$HF_FIXTURE_REPO" "${DL_ARGS[@]}"
) >/dev/null

# ─── [2/3] Upload via hosted OpenWeights CAS ────────────────────────────────────────
echo "[2/3] Uploading fixture through ${OPENWEIGHTS_CAS_URL}..."
# The demo namespace. Owner pre-creates this HF repo once OR points at their
# own scratch namespace; we keep the same basename as the source fixture for
# reviewer-recognizability.
DEMO_REPO="${OPENWEIGHTS_DEMO_REPO:-openweights-demo/$(basename "$HF_FIXTURE_REPO")}"
UP_ARGS=("$DEMO_REPO" "$TMP")
if [[ "$HF_FIXTURE_KIND" == "dataset" ]]; then
  UP_ARGS+=(--repo-type dataset)
fi
"$HF_BIN" upload "${UP_ARGS[@]}"

# ─── [3/3] Print receipt ───────────────────────────────────────────────────────
echo
echo "[3/3] Preload complete."
echo
echo "Next steps for the operator:"
echo "  1. Copy the 'file_id' printed above (hf upload receipt)."
echo "  2. Edit .env: set OPENWEIGHTS_FIXTURE_FILE_ID=<file_id>."
echo "  3. Re-source .env and run 'make smoke' — all 5 checks should pass."
echo
echo "Fixture URL:"
echo "  ${OPENWEIGHTS_CAS_URL}/v1/reconstructions/<file_id>"
