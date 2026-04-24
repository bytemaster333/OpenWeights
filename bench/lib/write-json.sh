#!/usr/bin/env bash
# bench/lib/write-json.sh
# Emits the two benchmark artifacts:
# - console/public/benchmarks.json (consumer: console/src/hooks/useStats.ts::useBenchmarks)
# - docs/benchmarks.md (reviewer-readable, : ONLY file permitted under docs/)
# Invocation (positional, all required; pass 'null' for un-measured cells):
# write-json.sh <size_bytes> <repo> <revision> <kind> \
# <sh_cold> <sh_warm> <sh_upload> \
# <hf_cold> <hf_warm> \
# <thesis_section_path_or_empty>
# The consumer (BenchmarksPage.tsx) expects THIS shape:
# { "generated_at": "<iso8601 or null>",
# "rows": [
# { "scenario": "cold-cache", "siahub_mbps": <n|null>, "hf_baseline_mbps": <n|null> },
# { "scenario": "warm-cache", ... },
# { "scenario": "upload", ... }
# ] }
# (We also embed a `fixture` + `methodology` block for humans reading the raw JSON.)

set -euo pipefail

if [[ $# -lt 9 ]]; then
  echo "usage: write-json.sh SIZE REPO REV KIND SH_COLD SH_WARM SH_UP HF_COLD HF_WARM [THESIS_SECTION_PATH]" >&2
  exit 2
fi

SIZE="$1" REPO="$2" REV="$3" KIND="$4"
SH_COLD="$5" SH_WARM="$6" SH_UP="$7"
HF_COLD="$8" HF_WARM="$9"
THESIS_SECTION_PATH="${10:-}"

# -----------------------------------------------------------------------------
# Derivation helpers. Take bytes + seconds, emit MB/s (decimal 10^6) rounded
# to 2 dp, or the literal string 'null' if either input is null/zero.
# -----------------------------------------------------------------------------
throughput_mbps() {
  local secs="$1"
  if [[ "$secs" == "null" || -z "$secs" ]]; then
    echo null
    return
  fi
  python3 -c "
import sys
b=float(sys.argv[1]); s=float(sys.argv[2])
print('null' if s <= 0 else f'{b/s/1e6:.2f}')
" "$SIZE" "$secs"
}

# Format seconds-as-JSON-number or 'null'.
jnum() {
  local v="$1"
  if [[ "$v" == "null" || -z "$v" ]]; then
    echo null
  else
    echo "$v"
  fi
}

# Format seconds for markdown display (or "—" for null).
mdnum() {
  local v="$1"
  if [[ "$v" == "null" || -z "$v" ]]; then
    echo "—"
  else
    echo "${v}s"
  fi
}

# Format throughput for markdown (or "—" for null).
mdmbps() {
  local v="$1"
  if [[ "$v" == "null" || -z "$v" ]]; then
    echo "—"
  else
    echo "${v} MB/s"
  fi
}

SH_COLD_MBPS=$(throughput_mbps "$SH_COLD")
SH_WARM_MBPS=$(throughput_mbps "$SH_WARM")
SH_UP_MBPS=$(throughput_mbps "$SH_UP")
HF_COLD_MBPS=$(throughput_mbps "$HF_COLD")
HF_WARM_MBPS=$(throughput_mbps "$HF_WARM")

NOW_ISO=$(date -u +%Y-%m-%dT%H:%M:%SZ)

# If every cell is null we treat `generated_at` as null so the console renders
# "Placeholder values shown..." rather than a misleading timestamp.
GENERATED_AT="\"$NOW_ISO\""
if [[ "$SH_COLD" == "null" && "$SH_WARM" == "null" && "$SH_UP" == "null" \
   && "$HF_COLD" == "null" && "$HF_WARM" == "null" ]]; then
  GENERATED_AT="null"
fi

# -----------------------------------------------------------------------------
# 1) console/public/benchmarks.json
# -----------------------------------------------------------------------------
OUT_JSON="console/public/benchmarks.json"
mkdir -p "$(dirname "$OUT_JSON")"

cat > "$OUT_JSON" <<EOF
{
  "generated_at": ${GENERATED_AT},
  "rows": [
    { "scenario": "cold-cache", "siahub_mbps": $(jnum "$SH_COLD_MBPS"), "hf_baseline_mbps": $(jnum "$HF_COLD_MBPS") },
    { "scenario": "warm-cache", "siahub_mbps": $(jnum "$SH_WARM_MBPS"), "hf_baseline_mbps": $(jnum "$HF_WARM_MBPS") },
    { "scenario": "upload",     "siahub_mbps": $(jnum "$SH_UP_MBPS"),   "hf_baseline_mbps": null }
  ],
  "fixture": {
    "repo": "$REPO",
    "revision": "$REV",
    "kind": "$KIND",
    "size_bytes": $SIZE
  },
  "methodology": {
    "trials_per_cell": 3,
    "aggregate": "median",
    "notes": "3 trials per cell, median only. See docs/benchmarks.md \u00a7Methodology for limits. Not a rigorous statistical study; no P10/P90/stddev."
  },
  "raw": {
    "siahub_cold_seconds": $(jnum "$SH_COLD"),
    "siahub_warm_seconds": $(jnum "$SH_WARM"),
    "siahub_upload_seconds": $(jnum "$SH_UP"),
    "hf_native_cold_seconds": $(jnum "$HF_COLD"),
    "hf_native_warm_seconds": $(jnum "$HF_WARM")
  }
}
EOF

# Validate with python (strict JSON).
python3 -c "import json,sys; json.load(open(sys.argv[1]))" "$OUT_JSON"

# -----------------------------------------------------------------------------
# 2) docs/benchmarks.md
# -----------------------------------------------------------------------------
OUT_MD="docs/benchmarks.md"
mkdir -p "$(dirname "$OUT_MD")"

# Human-friendly size (best-effort; falls back to raw bytes).
SIZE_HUMAN=$(python3 -c "
b=float('$SIZE')
units=[('GB',1e9),('MB',1e6),('KB',1e3)]
for u,f in units:
  if b >= f: print(f'{b/f:.1f} {u}'); break
else: print(f'{int(b)} bytes')
" 2>/dev/null || echo "$SIZE bytes")

cat > "$OUT_MD" <<EOF
# SiaHub benchmarks report

**Fixture:** \`$REPO\` (\`$KIND\`) @ \`$REV\` — ${SIZE} bytes (~${SIZE_HUMAN}).
**Measured:** ${NOW_ISO}.
**Methodology:** 3 trials per cell, median only. See §Methodology for the limits this imposes on reader interpretation.

## Results (median of 3 trials)

### Elapsed time

| Cell | SiaHub | HF-native |
|------|--------|-----------|
| cold-cache download | $(mdnum "$SH_COLD") | $(mdnum "$HF_COLD") |
| warm-cache download | $(mdnum "$SH_WARM") | $(mdnum "$HF_WARM") |
| upload              | $(mdnum "$SH_UP")   | (same endpoint — upload goes to hf.co in both stacks; see §Methodology) |

### Throughput (derived)

| Cell | SiaHub | HF-native |
|------|--------|-----------|
| cold-cache download | $(mdmbps "$SH_COLD_MBPS") | $(mdmbps "$HF_COLD_MBPS") |
| warm-cache download | $(mdmbps "$SH_WARM_MBPS") | $(mdmbps "$HF_WARM_MBPS") |
| upload              | $(mdmbps "$SH_UP_MBPS")   | — |

Throughput is computed as \`size_bytes / elapsed_seconds\` in decimal MB (10^6 bytes), matching how HF's own tooling reports network speeds. Entries shown as \`—\` are not yet measured — re-run \`make benchmark\` to populate.

## Methodology

- **Trials:** 3 per cell; median reported. No mean, no stddev, no P10/P90. Reader should treat these numbers as order-of-magnitude evidence that SiaHub serves bytes comparably to HF-native, not as precise percentile measurements. A rigorous study would run ≥5 trials back-to-back within a 60-second window with controlled network conditions — that is explicitly out of scope for v1 (see trim proposal §BENCH-05).
- **Cold-cache** = gateway LRU flushed before each trial via \`POST /admin/cache/flush\`. The xet-core local cache (\`~/.cache/huggingface/xet\`) is also purged between trials so the HTTP roundtrip actually touches Sia, not a local disk copy.
- **Warm-cache** = gateway LRU and xet-core local cache both primed by a prior download within the same run.
- **Upload comparison** — \`huggingface-cli upload\` talks to \`hf.co\` for the metadata CAS-commit regardless of which stack is serving xorbs. The SiaHub-vs-HF-native distinction only matters at the xorb PUT layer; the non-xorb metadata path is identical. We therefore publish SiaHub upload times only and explicitly omit a misleading "HF-native upload" column.
- **Hardware / network:** measurements were taken on the owner's server; spec + network conditions are documented in the repo README §Prerequisites. Reproducibility of the numbers depends on that environment — re-running \`make benchmark\` on a different host will produce different absolute numbers but the SiaHub-vs-HF-native ratio should stay within an order of magnitude.

## Reproducing

From a machine with the Compose stack already running (\`make up\`) and a funded wallet:

\`\`\`
# Measure only SiaHub (skip HF-native).
make benchmark STACK=siahub

# Measure SiaHub + HF-native baseline for comparison (default).
make benchmark

# Regenerate docs/benchmarks.md + console/public/benchmarks.json with no numbers
# (placeholder; useful for CI layout tests).
bash bench/run.sh --dry-run
\`\`\`

Environment variables the harness respects:

| Var | Purpose |
|-----|---------|
| \`SIAHUB_CAS_URL\` | Full URL to your CAS (e.g. \`https://cas.siahub.app\`). Required for the SiaHub cells. |
| \`SIAHUB_API_KEY\` | Bearer token minted via \`siahub.app/admin/keys\` (write scope). Required for upload cell. |
| \`STACK\` | \`siahub\` \| \`hf-native\` \| \`both\` (default: \`both\`). |
| \`BENCH_TRIALS\` | Override \`BENCH_TRIALS\` from \`bench/bench.config.sh\` (default: 3 per D-59). |

To swap the fixture: edit \`bench/bench.config.sh\` (\`HF_FIXTURE_REPO\`, \`HF_FIXTURE_REVISION\`, \`HF_FIXTURE_KIND\`) and re-run.

EOF

# Append §Thesis section if one was produced by the thesis fold-in step.
if [[ -n "${THESIS_SECTION_PATH}" && -f "${THESIS_SECTION_PATH}" ]]; then
  echo "" >> "$OUT_MD"
  cat "${THESIS_SECTION_PATH}" >> "$OUT_MD"
fi

echo "wrote $OUT_JSON"
echo "wrote $OUT_MD"
