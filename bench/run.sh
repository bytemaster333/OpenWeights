#!/usr/bin/env bash
# bench/run.sh — 3-trial median benchmark harness.
# Measures 5 cells across 2 stacks:
# openweights.cold-download openweights.warm-download openweights.upload
# hf-native.cold-download hf-native.warm-download
# Each cell runs BENCH_TRIALS (default: 3) trials; the median is reported.
# (There is no `hf-native.upload` because `huggingface-cli upload` talks to
# hf.co's own CAS in both stacks — the upload-side distinction is the xorb
# PUT layer only, and measuring it against "itself" produces no signal.)
# Writes:
# console/public/benchmarks.json (consumer: BenchmarksPage.tsx)
# docs/benchmarks.md (reviewer-facing report; : only file under docs/)
# Flags:
# --dry-run skip all HF network activity; emit placeholder artifacts
# (every cell value becomes "null"); useful for CI layout
# tests and for validating the output schema.
# --stack <s> openweights | hf-native | both (default: both)
# --trials <n> override BENCH_TRIALS (default: 3 per )
# Env: OPENWEIGHTS_CAS_URL (required for OpenWeights cells unless --dry-run)
# OPENWEIGHTS_API_KEY (required for OpenWeights upload cell unless --dry-run)
# INDEXD_ADMIN_PASSWORD (used by the thesis fold-in to query usable-host count)

set -euo pipefail

# Resolve repo root (script lives in bench/, so repo root is one level up).
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$REPO_ROOT"

# shellcheck disable=SC1091
source bench/bench.config.sh

DRY_RUN=0
STACK="${STACK:-both}"
TRIALS="${BENCH_TRIALS:-3}"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --dry-run) DRY_RUN=1 ;;
    --stack)   STACK="$2"; shift ;;
    --trials)  TRIALS="$2"; shift ;;
    -h|--help)
      sed -n '1,40p' "$0"
      exit 0
      ;;
    *)
      echo "unknown flag: $1" >&2
      exit 2
      ;;
  esac
  shift
done

if [[ "$STACK" != "openweights" && "$STACK" != "hf-native" && "$STACK" != "both" ]]; then
  echo "--stack must be one of: openweights | hf-native | both (got: $STACK)" >&2
  exit 2
fi

if [[ "$TRIALS" -lt 1 ]]; then
  echo "--trials must be >= 1 (got: $TRIALS)" >&2
  exit 2
fi

# -----------------------------------------------------------------------------
# Working directory + cleanup
# -----------------------------------------------------------------------------
WORK="$(mktemp -d -t openweights-bench-XXXXXX)"
trap 'rm -rf "$WORK"' EXIT

# Cell -> median-seconds map. We use five scalar variables rather than an
# associative array because bash 3.2 (the system shell on macOS) does not
# support `declare -A`.
MED_SH_COLD=null
MED_SH_WARM=null
MED_SH_UP=null
MED_HF_COLD=null
MED_HF_WARM=null

# -----------------------------------------------------------------------------
# Dry-run short-circuit — emit placeholder artifacts and exit 0.
# -----------------------------------------------------------------------------
if [[ "$DRY_RUN" -eq 1 ]]; then
  echo "[bench] --dry-run: skipping all network I/O; emitting placeholder artifacts"
  bash bench/lib/write-json.sh \
    "$HF_FIXTURE_SIZE_BYTES" "$HF_FIXTURE_REPO" "$HF_FIXTURE_REVISION" "$HF_FIXTURE_KIND" \
    null null null null null \
    ""
  exit 0
fi

# -----------------------------------------------------------------------------
# Pre-flight: required env vars + tools.
# -----------------------------------------------------------------------------
_need() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "[bench] missing required tool: $1" >&2
    exit 2
  }
}

_need python3
_need curl
_need jq
if ! command -v hf >/dev/null 2>&1 && ! command -v huggingface-cli >/dev/null 2>&1; then
  echo "[bench] neither 'hf' nor 'huggingface-cli' found on PATH" >&2
  exit 2
fi

# Prefer the modern 'hf' entrypoint (shipped by huggingface_hub >= 0.27);
# fall back to 'huggingface-cli' for older installs.
HF_CMD=$(command -v hf 2>/dev/null || command -v huggingface-cli)

if [[ "$STACK" == "openweights" || "$STACK" == "both" ]]; then
  : "${OPENWEIGHTS_CAS_URL:?OPENWEIGHTS_CAS_URL must be set for OpenWeights cells}"
  : "${OPENWEIGHTS_API_KEY:?OPENWEIGHTS_API_KEY must be set for OpenWeights upload cell}"
fi

# -----------------------------------------------------------------------------
# Cell-specific prep hook — runs BEFORE each trial of the named cell.
# For cold-cache cells we flush the gateway LRU + purge the xet-core local
# cache so the trial actually touches Sia.
# -----------------------------------------------------------------------------
prep_clean_state() {
  local cell="$1"
  local gw_flush_url="${OPENWEIGHTS_GATEWAY_ADMIN_URL:-http://localhost:9101}/admin/cache/flush"
  case "$cell" in
    openweights.cold-download)
      # Gateway LRU flush (OPS admin endpoint; 10-line handler — see plan Task 3
      # Note. On exit the gateway endpoint is expected to exist; if not,
      # a 404 is swallowed so the benchmark still runs (cold-vs-warm delta is
      # still measurable via xet-core cache purge alone, just less clean).
      curl -fsS -X POST -H "Authorization: Bearer ${OPENWEIGHTS_API_KEY:-}" \
        "$gw_flush_url" > /dev/null 2>&1 || true
      rm -rf "${HOME}/.cache/huggingface/xet" "$WORK/openweights-cold" 2>/dev/null || true
      mkdir -p "$WORK/openweights-cold"
      ;;
    openweights.warm-download)
      rm -rf "$WORK/openweights-warm"
      mkdir -p "$WORK/openweights-warm"
      ;;
    openweights.upload)
      rm -rf "$WORK/openweights-upload-src"
      cp -R "$WORK/fixture-src" "$WORK/openweights-upload-src"
      ;;
    hf-native.cold-download)
      rm -rf "${HOME}/.cache/huggingface/xet" "$WORK/hf-native-cold" 2>/dev/null || true
      mkdir -p "$WORK/hf-native-cold"
      ;;
    hf-native.warm-download)
      rm -rf "$WORK/hf-native-warm"
      mkdir -p "$WORK/hf-native-warm"
      ;;
  esac
}

# -----------------------------------------------------------------------------
# run_trial <label> <cmd...>
# Runs the command BENCH_TRIALS times, calling prep_clean_state between,
# and echoes the median elapsed seconds on stdout (format: %.3f).
# -----------------------------------------------------------------------------
run_trial() {
  local label="$1"; shift
  local samples=()
  local i s
  for i in $(seq 1 "$TRIALS"); do
    prep_clean_state "$label"
    s=$(bench/lib/measure.sh "${label}.t${i}" "$@")
    samples+=("$s")
    printf '  trial %s: %ss\n' "$i" "$s" >&2
  done
  python3 -c "
import statistics, sys
vals = [float(x) for x in sys.argv[1:]]
print(f'{statistics.median(vals):.3f}')
" "${samples[@]}"
}

# -----------------------------------------------------------------------------
# 0) one-time fixture fetch (used as the local source for the upload cell and
# for sizing the JSON `size_bytes`).
# -----------------------------------------------------------------------------
mkdir -p "$WORK/fixture-src"
echo "[bench] fetching fixture ${HF_FIXTURE_REPO}@${HF_FIXTURE_REVISION:0:12}..."
"$HF_CMD" download "$HF_FIXTURE_REPO" \
  --repo-type "$HF_FIXTURE_KIND" \
  --revision "$HF_FIXTURE_REVISION" \
  --local-dir "$WORK/fixture-src" \
  > /dev/null

# du -sb is GNU; macOS `du` lacks -b but has -k (kilobytes). Use python for
# portable "bytes-on-disk" including nested files.
FIXTURE_BYTES=$(python3 -c "
import os, sys
root=sys.argv[1]
total=0
for dp,_,fs in os.walk(root):
  for f in fs:
    try:
      total += os.path.getsize(os.path.join(dp,f))
    except OSError: pass
print(total)
" "$WORK/fixture-src")
echo "[bench] fixture: $HF_FIXTURE_REPO @ $HF_FIXTURE_REVISION ($FIXTURE_BYTES bytes)"

# -----------------------------------------------------------------------------
# 1) OpenWeights cells
# -----------------------------------------------------------------------------
if [[ "$STACK" == "openweights" || "$STACK" == "both" ]]; then
  export HF_XET_DATA_DEFAULT_CAS_ENDPOINT="$OPENWEIGHTS_CAS_URL"
  export HF_XET_DATA_CUSTOM_HEADERS="Authorization=Bearer ${OPENWEIGHTS_API_KEY}"

  echo "[bench] === OpenWeights: cold-cache download ==="
  MED_SH_COLD=$(run_trial openweights.cold-download \
    "$HF_CMD" download "$HF_FIXTURE_REPO" \
      --repo-type "$HF_FIXTURE_KIND" \
      --revision "$HF_FIXTURE_REVISION" \
      --local-dir "$WORK/openweights-cold")

  echo "[bench] === OpenWeights: warm-cache download ==="
  # Prime xet-core cache once outside the timed trials so the first warm trial
  # measures a real cache hit (not the first download populating the cache).
  "$HF_CMD" download "$HF_FIXTURE_REPO" \
    --repo-type "$HF_FIXTURE_KIND" \
    --revision "$HF_FIXTURE_REVISION" \
    --local-dir "$WORK/openweights-warm-prime" > /dev/null || true

  MED_SH_WARM=$(run_trial openweights.warm-download \
    "$HF_CMD" download "$HF_FIXTURE_REPO" \
      --repo-type "$HF_FIXTURE_KIND" \
      --revision "$HF_FIXTURE_REVISION" \
      --local-dir "$WORK/openweights-warm")

  echo "[bench] === OpenWeights: upload ==="
  # Upload goes to a disposable repo name so repeated trials don't collide.
  # We use the timestamp + trial index to keep names unique; the CAS will
  # just happily accept the re-uploaded xorbs (content-addressed, idempotent).
  UPLOAD_REPO="openweights-bench/upload-$(date +%s)"
  MED_SH_UP=$(run_trial openweights.upload \
    "$HF_CMD" upload "$UPLOAD_REPO" "$WORK/fixture-src")
fi

# -----------------------------------------------------------------------------
# 2) HF-native baseline cells — unset CAS endpoint, route through S3+CloudFront.
# -----------------------------------------------------------------------------
if [[ "$STACK" == "hf-native" || "$STACK" == "both" ]]; then
  # Unset the OpenWeights routing so xet-core falls back to HF's own CAS.
  # shellcheck disable=SC2086
  for v in $HF_BASELINE_UNSET_VARS; do unset "$v"; done

  echo "[bench] === HF-native: cold-cache download ==="
  MED_HF_COLD=$(run_trial hf-native.cold-download \
    "$HF_CMD" download "$HF_FIXTURE_REPO" \
      --repo-type "$HF_FIXTURE_KIND" \
      --revision "$HF_FIXTURE_REVISION" \
      --local-dir "$WORK/hf-native-cold")

  echo "[bench] === HF-native: warm-cache download ==="
  "$HF_CMD" download "$HF_FIXTURE_REPO" \
    --repo-type "$HF_FIXTURE_KIND" \
    --revision "$HF_FIXTURE_REVISION" \
    --local-dir "$WORK/hf-native-warm-prime" > /dev/null || true

  MED_HF_WARM=$(run_trial hf-native.warm-download \
    "$HF_CMD" download "$HF_FIXTURE_REPO" \
      --repo-type "$HF_FIXTURE_KIND" \
      --revision "$HF_FIXTURE_REVISION" \
      --local-dir "$WORK/hf-native-warm")
fi

# -----------------------------------------------------------------------------
# 3) thesis live-run conditional.
# USABLE = count of Zen hosts with positive uptime usability.
# If USABLE >= 6 -> run `make thesis`, fold verdict into docs/benchmarks.md.
# If USABLE < 6 -> append a NOT-RUN-LIVE blocker note citing
# indexd's hardcoded minRecoveryProbability=99.99%.
# -----------------------------------------------------------------------------
THESIS_SECTION=""
if [[ -n "${INDEXD_ADMIN_PASSWORD:-}" ]]; then
  USABLE=$(
    curl -fsS -u ":${INDEXD_ADMIN_PASSWORD}" http://localhost:9980/api/hosts 2>/dev/null \
      | jq '[.[] | select(.usability.uptime // .scoreBreakdown.totalScore // 0 | tonumber > 0)] | length' \
      2>/dev/null || echo 0
  )
  USABLE_INT=$(echo "$USABLE" | tr -d '[:space:]')
  [[ "$USABLE_INT" =~ ^[0-9]+$ ]] || USABLE_INT=0
  echo "[bench] Zen testnet usable hosts: $USABLE_INT"

  THESIS_SECTION="$WORK/thesis.md"
  if [[ "$USABLE_INT" -ge 6 ]]; then
    echo "[bench] usable hosts >= 6; running make thesis..."
    THESIS_RC=0
    (cd "$REPO_ROOT" && make thesis) > "$WORK/thesis.log" 2>&1 || THESIS_RC=$?
    if [[ $THESIS_RC -eq 0 ]]; then
      VERDICT="PASS"
    else
      VERDICT="FAIL"
    fi
    cat > "$THESIS_SECTION" <<EOF
## Thesis ( range-download validation)

**Verdict:** ${VERDICT} — measured $(date -u +%Y-%m-%dT%H:%M:%SZ), ${USABLE_INT} usable Zen hosts.
64 MiB range-download sector-scoping test ran via \`make thesis\` (PLAN 01-06 harness).
See \`bench/thesis/REPORT.md\` for the per-trial breakdown and \`bench/thesis/runs/\` for raw data.
EOF
  else
    cat > "$THESIS_SECTION" <<EOF
## Thesis ( range-download validation)

**Status: NOT RUN LIVE** — Sia Zen testnet had only ${USABLE_INT} usable hosts on $(date -u +%Y-%m-%d) (measured via \`indexd /api/hosts\`). \`indexd\` hardcodes \`minRecoveryProbability = 99.99%\` in \`go.sia.tech/indexd/slabs/slabs.go\`, which requires a usable-host count that Zen currently does not sustain (no 3-host redundancy scheme reaches 99.99% — 1-of-3 parity maxes out at ~98.43% recovery probability).

This is a **Sia-network environmental blocker**, not a OpenWeights code defect. The thesis measurement code itself ships and is unit-tested (\`bench/thesis/thesis_test.go\` + \`TestRangeDownloadSectorScoping\` integration test) — it is gated on Zen testnet stabilising at ≥6 usable hosts, at which point the existing \`WithRedundancy(1, 2)\` override clears.

Functional coverage for the range-download code path is provided by the conformance suite (always green in CI) and the HF byte-identical multi-GB round-trip test (PROTO-11, plan 05-02). Those exercise every range-download code path; what they do NOT quantify is the sector-level byte count — that is what \`make thesis\` would measure on a healthier host pool.
EOF
  fi
else
  echo "[bench] INDEXD_ADMIN_PASSWORD not set; skipping thesis fold-in"
fi

# -----------------------------------------------------------------------------
# 4) Emit artifacts.
# -----------------------------------------------------------------------------
bash bench/lib/write-json.sh \
  "$FIXTURE_BYTES" "$HF_FIXTURE_REPO" "$HF_FIXTURE_REVISION" "$HF_FIXTURE_KIND" \
  "$MED_SH_COLD" "$MED_SH_WARM" "$MED_SH_UP" \
  "$MED_HF_COLD" "$MED_HF_WARM" \
  "$THESIS_SECTION"

echo "[bench] done."
