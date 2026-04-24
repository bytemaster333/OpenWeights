#!/usr/bin/env bash
# bench/bench.config.sh — pinned fixture model source-of-truth.
# : one pinned-revision fixture used by EVERY harness that touches HF:
# - bench/run.sh ( — this directory's harness)
# - tests/hf-roundtrip/run.sh ( — byte-identical multi-GB round-trip)
# - ops/smoke.sh ( — hosted-demo post-deploy smoke)
# Probe 1 resolution (see §Task 1):
# Candidate considered: bert-base-uncased (mainstream; reviewer-recognizable),
# HuggingFaceH4/zephyr-7b-beta (~4.4 GB), sentence-transformers/all-MiniLM-L6-v2
# (~90 MB, mainstream). HF public API reports xetEnabled=null for all three
# (field is not reliably exposed), so we cannot programmatically confirm
# Xet-enablement at planner-execute time.
# Picked: xet-team/xet-spec-reference-files — because:
# (a) Xet-team-authored → Xet-enablement guaranteed (doesn't require API probe);
# (b) Already pinned in Makefile `conformance-fixtures` target at the exact
# same SHA (single source of truth across conformance + bench);
# (c) 78 MB total — trivially fits gateway LRU cache (20 GB default)
# AND the owner's server disk budget (≥50 GB headroom per deploy pre-check).
# Probe 4 resolved: fixture << 10 GB risk ceiling.
# (d) The grant-reviewer "recognizability" argument for bert-base-uncased is
# subjective; reviewer-useful signal is the MEASUREMENT methodology, not
# the model name.
# If a future planner swaps this for a mainstream model, update:
# - HF_FIXTURE_REPO
# - HF_FIXTURE_REVISION (MUST be a 40-char hex commit SHA; rerun the benchmark)
# - HF_FIXTURE_SIZE_BYTES (updated by bench/run.sh on first real run)
# - HF_FIXTURE_KIND (datasets/ prefix for HF `hf download --repo-type`)
# and re-run both (05-02) and BENCH-REPORT (05-03).

# shellcheck disable=SC2034 # sourced by other scripts; exports are intentional

export HF_FIXTURE_REPO="xet-team/xet-spec-reference-files"
export HF_FIXTURE_KIND="dataset"                            # datasets use --repo-type dataset
export HF_FIXTURE_REVISION="18bf9173fb2ca80ab3a6fdff81119ff61be7e7dd"
export HF_FIXTURE_SIZE_BYTES="78265061"                     # ~78 MB; refreshed by bench/run.sh first run
export HF_FIXTURE_DESCRIPTION="xet-spec reference fixture files, pinned revision (same SHA as conformance harness)"

# HF-native comparison endpoint — unsetting HF_XET_DATA_DEFAULT_CAS_ENDPOINT
# routes xet-core through HF's own S3+CloudFront CAS. This is the
# baseline column (trim proposal subsumed it into BENCH-REPORT).
export HF_BASELINE_UNSET_VARS="HF_XET_DATA_DEFAULT_CAS_ENDPOINT HF_XET_DATA_CUSTOM_HEADERS"

# Bench trial count — : 3 trials, median only. No P10/P90/stddev.
export BENCH_TRIALS=3
