#!/usr/bin/env bash
# bench/lib/measure.sh <label> <cmd...>
# Runs <cmd> once, emits elapsed-seconds on stdout (format: %.3f).
# Caller is responsible for cache/state setup BETWEEN trials (see
# prep_clean_state in bench/run.sh). On any non-zero exit from <cmd>,
# stderr of <cmd> is dumped and this script exits with the same code.
# Env: BENCH_STDERR_LOG (optional) — path to append wrapped-command stderr.
# Defaults to /tmp/bench_stderr.<pid>.

set -euo pipefail

if [[ $# -lt 2 ]]; then
  echo "usage: measure.sh <label> <cmd...>" >&2
  exit 2
fi

LABEL="$1"
shift

STDERR_LOG="${BENCH_STDERR_LOG:-/tmp/bench_stderr.$$}"

# Portable high-resolution timer. GNU `date +%s.%N` works on Linux;
# macOS / BSD `date` lacks %N, so fall back to Python when unavailable.
_now() {
  local t
  t=$(date +%s.%N 2>/dev/null) || true
  if [[ -z "${t}" || "${t}" == *N* ]]; then
    python3 -c 'import time; print(f"{time.time():.6f}")'
  else
    echo "${t}"
  fi
}

START=$(_now)
set +e
"$@" > /dev/null 2>"${STDERR_LOG}"
RC=$?
set -e
END=$(_now)

if [[ $RC -ne 0 ]]; then
  echo "[measure] ${LABEL} FAIL rc=${RC}" >&2
  if [[ -s "${STDERR_LOG}" ]]; then
    echo "[measure] ${LABEL} stderr:" >&2
    sed 's/^/  /' "${STDERR_LOG}" >&2
  fi
  exit $RC
fi

python3 -c "import sys; s=float(sys.argv[1]); e=float(sys.argv[2]); print(f'{e-s:.3f}')" "$START" "$END"
