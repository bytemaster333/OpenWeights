#!/usr/bin/env bash
# Standalone-mode HF round-trip: proves a byte-identical upload -> (fresh cache)
# -> download through OpenWeights, with the CAS acting as the Hub
# (HF_ENDPOINT points at the CAS). This is the M2 acceptance flow from
# proposal.md: `hf upload` then `hf download` then sha256 compare, no
# huggingface.co involvement.
#
# Unlike the old env-var-only harness, this:
#   * routes the client at the CAS via HF_ENDPOINT (the mode the README documents
#     and the only one that actually redirects an HF-integrated upload),
#   * downloads the SAME repo it just uploaded,
#   * clears the cache before the download leg so bytes are truly reconstructed
#     from OpenWeights (Xet reconstruction -> gateway -> Sia), and
#   * hard-fails on any upload/download error or SHA mismatch.
#
# Env:
#   CAS_URL              CAS base URL / HF_ENDPOINT (default http://localhost:28080)
#   OPENWEIGHTS_API_KEY  a key with BOTH upload+download scope (see below)
#   HF_CLI               hf binary (default: `hf` on PATH)
#   REPO                 target repo (default openweights-e2e/roundtrip-<pid>)
#   KEEP                 set to keep the temp work dir for inspection
# Exit: 0 byte-identical; 1 failure/mismatch.

set -euo pipefail

CAS="${CAS_URL:-http://localhost:28080}"
TOKEN="${OPENWEIGHTS_API_KEY:?OPENWEIGHTS_API_KEY required (mint via scripts/issue-test-key.sh; needs upload+download scope)}"
HF_CLI="${HF_CLI:-hf}"
REPO="${REPO:-openweights-e2e/roundtrip-$$}"

command -v "$HF_CLI" >/dev/null 2>&1 || { echo "FAIL: '$HF_CLI' not found on PATH" >&2; exit 1; }

WORK="$(mktemp -d)"
cleanup() { [[ -n "${KEEP:-}" ]] || rm -rf "$WORK"; }
trap cleanup EXIT

SRC="$WORK/src"; DST="$WORK/dl"; CACHE="$WORK/cache"
mkdir -p "$SRC"

# Deterministic-per-run fixture: a multi-MiB binary (exercises the Xet/xorb
# path) plus a small text file (exercises the LFS/regular path).
head -c 4194304 /dev/urandom > "$SRC/weights.bin"
printf 'openweights standalone round-trip fixture\n' > "$SRC/README.md"

sha_tree() { (cd "$1" && find . -type f -not -path '*/.*' | sort | xargs shasum -a 256 | awk '{print $1"  "$2}'); }
SRC_SHA="$(sha_tree "$SRC")"

echo "[1/3] hf upload -> $CAS ($REPO)"
HF_ENDPOINT="$CAS" HF_TOKEN="$TOKEN" "$HF_CLI" upload "$REPO" "$SRC" --repo-type model >/dev/null

echo "[2/3] hf download from a FRESH cache"
HF_ENDPOINT="$CAS" HF_TOKEN="$TOKEN" HF_HOME="$CACHE" HF_XET_CACHE="$CACHE/xet" \
  "$HF_CLI" download "$REPO" --repo-type model --local-dir "$DST" >/dev/null
DST_SHA="$(sha_tree "$DST")"

echo "[3/3] compare sha256 of every file"
if [[ "$SRC_SHA" != "$DST_SHA" ]]; then
  echo "FAIL: byte mismatch" >&2
  echo "--- uploaded ---"; echo "$SRC_SHA"
  echo "--- downloaded ---"; echo "$DST_SHA"
  exit 1
fi
echo "OK: byte-identical round-trip"
echo "$SRC_SHA"
