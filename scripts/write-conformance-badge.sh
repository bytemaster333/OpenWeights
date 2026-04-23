#!/usr/bin/env bash
# Plan 05-01 Task 4 — write console/public/conformance-badge.json.
#
# Usage: write-conformance-badge.sh <pass|fail>
#
# Output shape is frozen by D-72:
#   { "status":   "pass" | "fail" | "unknown",
#     "last_run": "<RFC3339 UTC, seconds precision>",
#     "commit":   "<git sha | 'unknown'>",
#     "run_url":  "<GH Actions run URL | ''>" }
#
# The Phase 4 console hook (console/src/hooks/useConformance.ts) tolerates
# unknown keys and missing optional fields — status + last_run are the only
# required shape invariants.

set -euo pipefail

STATUS="${1:?usage: write-conformance-badge.sh <pass|fail>}"
case "$STATUS" in
  pass|fail) ;;
  *)
    echo "status must be 'pass' or 'fail' (got: $STATUS)" >&2
    exit 2
    ;;
esac

LAST_RUN="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
COMMIT="${GH_COMMIT:-unknown}"
RUN_URL="${GH_RUN_URL:-}"

TARGET="${TARGET:-console/public/conformance-badge.json}"

# Emit via python3 to guarantee JSON-correct escaping of run_url / commit
# even if a caller ever passes values with embedded quotes.
python3 - "$STATUS" "$LAST_RUN" "$COMMIT" "$RUN_URL" "$TARGET" <<'PY'
import json, sys
status, last_run, commit, run_url, target = sys.argv[1:6]
payload = {
    "status": status,
    "last_run": last_run,
    "commit": commit,
    "run_url": run_url,
}
with open(target, "w") as f:
    json.dump(payload, f, indent=2)
    f.write("\n")
print(f"wrote {target}: status={status} last_run={last_run} commit={commit}")
PY
