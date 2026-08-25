# OpenWeights root Makefile.
# Single entry point for every operator task.
# Targets whose recipes depend on code from later plans emit a clear
# "not-yet-implemented" message and exit 2 (distinct from failure exit 1).

.PHONY: setup bootstrap bootstrap-reset up down thesis smoke compose-smoke verify test-unit clean \
 help \
 cas-build cas-check cas-clippy cas-run cas-image cas-up \
 gateway-build gateway-check gateway-vet gateway-run gateway-test gateway-image gateway-up \
 console-install console-dev console-build console-check console-test console-image console-up \
 benchmark benchmark-report benchmark-dry-run \
 integration-hf-roundtrip integration-hf-roundtrip-dry-run integration-hf-roundtrip-down \
 deploy deploy-smoke preload-fixture

GO := go
COMPOSE := docker compose -f ops/docker-compose.yml --env-file .env
BENCH := cd bench &&

# -----------------------------------------------------------------------------
# High-level operator entry points
# -----------------------------------------------------------------------------

help:
	@echo "OpenWeights Makefile targets:"
	@echo " make bootstrap From-zero-to-running wizard (first-time setup)"
	@echo " make bootstrap-reset Remove .env for a clean re-bootstrap"
	@echo " make up docker compose up -d"
	@echo " make down docker compose down"
	@echo " make thesis Sia range-download thesis measurement ( gate)"
	@echo " make smoke 1 MiB round-trip test against the external indexer"
	@echo " make compose-smoke Verify Compose healthchecks pass end-to-end"
	@echo " make verify Full suite: unit + thesis + smoke + compose-smoke"
	@echo " make test-unit Fast Go unit tests (no network)"
	@echo " make clean Tear down Compose + remove volumes (DESTRUCTIVE)"
	@echo " ---"
	@echo " make console-install pnpm install (; frozen lockfile)"
	@echo " make console-dev pnpm run dev (Vite dev server :5173)"
	@echo " make console-build pnpm run build (tsc + Vite)"
	@echo " make console-check pnpm run check ( CI gate: biome + tsc + vitest)"
	@echo " make console-test pnpm run test (Vitest)"
	@echo " make console-image docker build openweights-console"
	@echo " make console-up docker compose up -d openweights-console"
	@echo " ---"
	@echo " make benchmark 3-trial median throughput report ( gate #4)"
	@echo " make benchmark-dry-run Regenerate benchmark artifacts with placeholder nulls"
	@echo " ---"
	@echo " make integration-hf-roundtrip HF byte-identical round-trip through Caddy ( gate #2)"
	@echo " make integration-hf-roundtrip-dry-run Lint harness scripts + validate compose overlay (no stack)"
	@echo " make integration-hf-roundtrip-down Tear down the Caddy-fronted CI stack"
	@echo " ---"
	@echo " make deploy Hosted demo deploy (manual; ) — pulls + brings stack up + smokes"
	@echo " make deploy-smoke Post-deploy objective smoke (ops/smoke.sh; /)"
	@echo " make preload-fixture Seed the hosted demo with the pinned fixture model "

setup:
	@echo "setup: interactive .env wizard (prompts for indexer + admin password + recovery phrase, generates the rest)..."
	@mkdir -p bin
	$(BENCH) $(GO) build -o "$(CURDIR)/bin/bootstrap" ./bootstrap
	./bin/bootstrap -env-only

bootstrap:
	@echo "bootstrap: running wizard (writes .env, brings up the stack, registers the app on the indexer, smoke-tests)..."
	@mkdir -p bin
	$(BENCH) $(GO) build -o "$(CURDIR)/bin/bootstrap" ./bootstrap
	./bin/bootstrap

up:
	$(COMPOSE) up -d

down:
	$(COMPOSE) down

# -----------------------------------------------------------------------------
# measurement targets
# -----------------------------------------------------------------------------

thesis:
	@echo "thesis: running measurement against external indexer (PASS=0, FAIL=3 informational per )..."
	@set +e; $(BENCH) $(GO) run ./thesis; rc=$$?; \
	 if [ $$rc -eq 0 ]; then echo "thesis: PASS"; exit 0; \
	 elif [ $$rc -eq 3 ]; then echo "thesis: FAIL (informational — see bench/thesis/REPORT.md + CONTEXT )"; exit 0; \
	 else echo "thesis: hard error rc=$$rc"; exit $$rc; fi
	@echo "thesis: report written; see bench/thesis/REPORT.md"

smoke:
	$(BENCH) $(GO) run ./smoke

compose-smoke:
	@echo "compose-smoke: exercising full stack healthcheck gating..."
	$(BENCH) $(GO) test -tags=integration -timeout=35m ./compose-smoke/...

verify: test-unit
	@echo "verify: running full suite (thesis + smoke + compose-smoke)"
	$(MAKE) thesis
	$(MAKE) compose-smoke
	$(MAKE) smoke

test-unit:
	$(BENCH) $(GO) test -short ./...

# -----------------------------------------------------------------------------
# bootstrap-reset — remove wizard-generated local state for a clean re-bootstrap.
# Keeps Compose volumes intact (xorbs stay pinned on Sia; run `make clean` for
# a truly destructive tear-down).
# -----------------------------------------------------------------------------

bootstrap-reset:
	rm -f .env
	@echo "bootstrap-reset: .env removed; run 'make bootstrap' to redo."

# -----------------------------------------------------------------------------
# Destructive
# -----------------------------------------------------------------------------

clean:
	$(COMPOSE) down -v
	rm -rf bench/*/runs

# -----------------------------------------------------------------------------
# : openweights-cas
# -----------------------------------------------------------------------------
.PHONY: cas-build cas-check cas-clippy cas-run cas-image cas-up

cas-build:
	cd cas && cargo build --release --bin openweights-cas

cas-check:
	cd cas && cargo check --workspace

cas-clippy:
	cd cas && cargo clippy --workspace --all-targets -- -D warnings

cas-run:
	cd cas && cargo run --bin openweights-cas

cas-image:
	docker compose -f ops/docker-compose.yml build openweights-cas

cas-up: cas-image
	docker compose -f ops/docker-compose.yml up -d openweights-cas

# -----------------------------------------------------------------------------
# : openweights-gateway
# Plans 03-01..03-07. Wave 1 scaffold ships /health + stub /xorb/{hash} (501)
# + HMAC-SHA256 signed-URL verifier (cross-language vectors green).
# -----------------------------------------------------------------------------
.PHONY: gateway-build gateway-check gateway-vet gateway-run gateway-test gateway-image gateway-up

gateway-build:
	@mkdir -p gateway/bin
	cd gateway && $(GO) build -o bin/openweights-gateway .

gateway-check:
	cd gateway && $(GO) build ./...

gateway-vet:
	cd gateway && $(GO) vet ./...

gateway-test:
	cd gateway && $(GO) test ./...

gateway-run:
	cd gateway && $(GO) run .

gateway-image:
	docker compose -f ops/docker-compose.yml build openweights-gateway

gateway-up: gateway-image
	docker compose -f ops/docker-compose.yml up -d openweights-gateway

# -----------------------------------------------------------------------------
# : openweights-console
# Plans 04-02..04-10. First commit = shadcn init preset b7C9wTXYe (04-02).
# `console-check` is the CI gate (: biome check + tsc + vitest).
# -----------------------------------------------------------------------------
.PHONY: console-install console-dev console-build console-check console-test console-image console-up

console-install:
	cd console && corepack enable && pnpm install --frozen-lockfile

console-dev:
	cd console && pnpm run dev

console-build:
	cd console && pnpm run build

console-check:
	cd console && pnpm run check

console-test:
	cd console && pnpm run test

console-image:
	docker compose -f ops/docker-compose.yml build openweights-console

console-up: console-image
	docker compose -f ops/docker-compose.yml up -d openweights-console

# -----------------------------------------------------------------------------
# : openweights-conformance — Xet protocol end-to-end harness.
# (wave 6). Drives the full CAS via xet_client = "=1.5.1" as a
# dev-dep (never a runtime dep). See conformance/Cargo.toml for the pin
# rationale + T-02-10-06 guard.
# -----------------------------------------------------------------------------
.PHONY: conformance conformance-fixtures conformance-check conformance-clippy conformance-local

conformance-fixtures:
	@if [ ! -f conformance/fixtures/eea25d6ee393ccae385820daed127b96ef0ea034dfb7cf6da3a950ce334b7632.xorb ]; then \
	 echo "Fetching conformance fixtures from xet-team/xet-spec-reference-files@18bf9173fb..."; \
	 cd conformance && git lfs clone https://huggingface.co/datasets/xet-team/xet-spec-reference-files \
	 --revision 18bf9173fb2ca80ab3a6fdff81119ff61be7e7dd fixtures/; \
	else \
	 echo "conformance-fixtures: already present (skipping)"; \
	fi

# Unit-level + compile check; never touches Docker.
conformance-check:
	cd conformance && cargo check --tests

conformance-clippy:
	cd conformance && cargo clippy --all-targets -- -D warnings

# Full run. Requires a openweights-cas image built via `make cas-image` AND the
# fixtures cloned. Individual tests skip with eprintln when preconditions
# aren't met — the invocation itself never fails on a fresh clone.
conformance: conformance-fixtures
	cd conformance && cargo test --release

# — run the conformance crate against a live Compose stack with the CI overlay
# (V2_RECONSTRUCTION_ENABLED=true), waiting for every service to report healthy,
# pre-tagging `openweights-cas:conformance`
# for the testcontainers harness, runs the conformance crate, then writes
# console/public/conformance-badge.json via the same script CI uses. Tear
# the stack down with `make down` when you're finished.
conformance-local: conformance-fixtures
	@echo "conformance-local: building + tagging openweights-cas image for conformance harness..."
	docker compose -f ops/docker-compose.yml --env-file .env build openweights-cas
	@docker tag ops-openweights-cas:latest openweights-cas:conformance 2>/dev/null \
	 || docker tag openweights-cas:latest openweights-cas:conformance
	@echo "conformance-local: bringing up compose stack (base + CI overlay)..."
	docker compose -f ops/docker-compose.yml -f ops/docker-compose.ci.yml --env-file .env up -d
	@echo "conformance-local: waiting for stack healthy..."
	bash scripts/wait-for-stack-healthy.sh
	@echo "conformance-local: running conformance crate..."
	cd conformance && cargo test --release
	@echo "conformance-local: writing PASS badge..."
	GH_COMMIT="$$(git rev-parse HEAD 2>/dev/null || echo local)" \
	 GH_RUN_URL="local" \
	 bash scripts/write-conformance-badge.sh pass
	@echo "conformance-local: PASS — stack still running; 'make down' to tear down."

# -----------------------------------------------------------------------------
# : benchmarks report — 3-trial median harness.
#. Writes console/public/benchmarks.json + docs/benchmarks.md.
# STACK=both (default) | openweights | hf-native.
# Requires OPENWEIGHTS_CAS_URL + OPENWEIGHTS_API_KEY in env for OpenWeights cells; HF CLI
# (`hf` or `huggingface-cli`) on PATH.
# -----------------------------------------------------------------------------
.PHONY: benchmark benchmark-report benchmark-dry-run

benchmark:
	STACK=$${STACK:-both} bash bench/run.sh --stack $${STACK:-both}

# Alias for clarity when invoked from docs / submission instructions.
benchmark-report: benchmark

# Regenerate the JSON+MD artifacts with all-null values (placeholder). Useful
# for CI layout tests + when the fixture/schema changes but we have no live
# stack to measure against yet.
benchmark-dry-run:
	bash bench/run.sh --dry-run

# -----------------------------------------------------------------------------
# : HF byte-identical round-trip integration test (Gate #2).
# Validates the Caddy-fronted stack end-to-end. Expects .env present
# (run `make bootstrap` first) AND a fresh
# Postgres with `openweights` schema — the helper mints a test API key via
# direct psql INSERT (scripts/issue-test-key.sh).
#
# STACK=openweights-ci (default) | custom - passed to docker compose as an
# ancillary label; no effect on
# functionality today.
# -----------------------------------------------------------------------------

# Compose file list shared by the three roundtrip-* targets.
CADDY_COMPOSE := docker compose \
 -f ops/docker-compose.yml \
 -f ops/docker-compose.ci.yml \
 -f ops/docker-compose.caddy.yml \
 --env-file .env

integration-hf-roundtrip-dry-run:
	@echo "integration-hf-roundtrip-dry-run: linting harness scripts..."
	bash -n tests/hf-roundtrip/run.sh
	bash -n tests/hf-roundtrip/verify-range-integrity.sh
	bash -n scripts/issue-test-key.sh
	@echo "integration-hf-roundtrip-dry-run: validating 3-way compose overlay merge..."
	@POSTGRES_SUPERUSER_PASSWORD=dry \
	 OPENWEIGHTS_POSTGRES_PASSWORD=dry \
	 OPENWEIGHTS_GW_POSTGRES_PASSWORD=dry \
	 REDIS_PASSWORD=dry \
	 docker compose -f ops/docker-compose.yml -f ops/docker-compose.ci.yml -f ops/docker-compose.caddy.yml config >/dev/null \
	 && echo "overlay merges clean"
	@echo "integration-hf-roundtrip-dry-run: OK"

integration-hf-roundtrip: integration-hf-roundtrip-dry-run
	@echo "integration-hf-roundtrip: bringing up Caddy-fronted CI stack..."
	$(CADDY_COMPOSE) up -d --build
	@echo "integration-hf-roundtrip: waiting for stack healthy..."
	bash scripts/wait-for-stack-healthy.sh
	@echo "integration-hf-roundtrip: issuing test API key..."
	@KEY=$$(bash scripts/issue-test-key.sh --scope write); \
	 if [ -z "$$KEY" ]; then echo "issue-test-key failed"; exit 1; fi; \
	 . bench/bench.config.sh; \
	 docker build -t openweights-hf-roundtrip:ci tests/hf-roundtrip; \
	 docker run --rm --network host \
	 -e CAS_BASE_URL=http://localhost:8090/cas \
	 -e GATEWAY_BASE_URL=http://localhost:8090/gateway \
	 -e OPENWEIGHTS_API_KEY="$$KEY" \
	 -e HF_FIXTURE_REPO \
	 -e HF_FIXTURE_REVISION \
	 -e HF_FIXTURE_KIND \
	 openweights-hf-roundtrip:ci
	@echo "integration-hf-roundtrip: PASS — run 'make integration-hf-roundtrip-down' to tear down."

integration-hf-roundtrip-down:
	$(CADDY_COMPOSE) down -v

# -----------------------------------------------------------------------------
# : Hosted demo deploy (, manual deploy).
#
# Autonomous: FALSE. Never invoked from CI. The operator runs these commands
# while SSH'd into the owner's server AFTER `git pull` brings the repo to the
# desired HEAD. See for the
# full 10-step runbook (DNS →.env → staging cert validation → prod deploy →
# wallet funding → first API key → fixture preload → smoke).
#
# The existing `smoke` target ( Sia range-download smoke) is NOT
# overwritten — hosted-demo smoke is `deploy-smoke` to avoid breaking the
# Phase-1 wiring that still references `make smoke` via `make verify`.
# -----------------------------------------------------------------------------

PROD_COMPOSE := docker compose \
 -f ops/docker-compose.yml \
 -f ops/docker-compose.prod.yml \
 --env-file .env

# Pre-conditions:
# - SSH'd into the owner's server.
# - `git pull` brought the repo to the commit you want deployed.
# - `.env` exists (chmod 0600) with every variable from both
# `ops/.env.example` and `ops/.env.prod.example` populated.
# - DNS A-records for openweights.app + cas.openweights.app point at this server.
# - Ports 80 + 443 reachable from the public internet (Caddy ACME challenge).
deploy:
	@test -f .env || (echo "ERROR: .env missing. Copy from ops/.env.prod.example (plus ops/.env.example) and fill in every value." && exit 1)
	@test -f ops/Caddyfile || (echo "ERROR: ops/Caddyfile missing — did you git pull?" && exit 1)
	@test -f ops/docker-compose.prod.yml || (echo "ERROR: ops/docker-compose.prod.yml missing — did you git pull?" && exit 1)
	@echo "deploy: pulling latest images..."
	$(PROD_COMPOSE) pull
	@echo "deploy: building openweights-* service images from repo HEAD..."
	$(PROD_COMPOSE) build
	@echo "deploy: bringing stack up (detached)..."
	$(PROD_COMPOSE) up -d
	@echo "deploy: waiting for stack healthy..."
	bash scripts/wait-for-stack-healthy.sh
	@echo "deploy: stack healthy. Run 'make deploy-smoke' to verify end-to-end reachability."

# Objective post-deploy smoke test against the LIVE hosted demo.
# Equivalent to the cut tester-recruitment criterion.
deploy-smoke:
	bash ops/smoke.sh $${OPENWEIGHTS_DOMAIN:-openweights.app}

# One-shot fixture preload. Run AFTER a write-scoped API key is
# minted in the console. See ops/preload-fixture.sh for pre-conditions.
preload-fixture:
	bash ops/preload-fixture.sh
