# SiaHub root Makefile.
# Single entry point for every operator task (CONTEXT D-03 + D-07).
# Targets whose recipes depend on code from later plans emit a clear
# "not-yet-implemented" message and exit 2 (distinct from failure exit 1).

.PHONY: bootstrap bootstrap-reset up down thesis smoke compose-smoke verify test-unit clean \
        build-readiness-probe help a3-verify

GO      := go
COMPOSE := docker compose -f ops/docker-compose.yml --env-file .env
BENCH   := cd bench &&

# -----------------------------------------------------------------------------
# High-level operator entry points
# -----------------------------------------------------------------------------

help:
	@echo "SiaHub Makefile targets:"
	@echo "  make bootstrap         From-zero-to-running wizard (first-time setup)"
	@echo "  make bootstrap-reset   Remove .env + ops/indexd.yml for a clean re-bootstrap"
	@echo "  make up                docker compose up -d"
	@echo "  make down              docker compose down"
	@echo "  make thesis            Sia range-download thesis measurement (Phase 1 gate)"
	@echo "  make smoke             1 MiB round-trip test against live indexd"
	@echo "  make compose-smoke     Verify Compose healthchecks pass end-to-end"
	@echo "  make verify            Full suite: unit + thesis + smoke + compose-smoke"
	@echo "  make test-unit         Fast Go unit tests (no network)"
	@echo "  make clean             Tear down Compose + remove volumes (DESTRUCTIVE)"

bootstrap: bench/compose-smoke/readiness/bin/readiness
	@echo "bootstrap: running wizard (renders ops/indexd.yml + brings up stack + funds wallet + smoke)..."
	@mkdir -p bin
	$(BENCH) $(GO) build -o "$(CURDIR)/bin/bootstrap" ./bootstrap
	./bin/bootstrap

up:
	$(COMPOSE) up -d

down:
	$(COMPOSE) down

# -----------------------------------------------------------------------------
# Phase 1 measurement targets
# -----------------------------------------------------------------------------

thesis: bench/compose-smoke/readiness/bin/readiness
	$(COMPOSE) up -d postgres indexd redis
	@echo "thesis: running measurement (PASS=0, FAIL=3 informational per D-02)..."
	@set +e; $(BENCH) $(GO) run ./thesis; rc=$$?; \
	  if [ $$rc -eq 0 ]; then echo "thesis: PASS"; exit 0; \
	  elif [ $$rc -eq 3 ]; then echo "thesis: FAIL (informational — see bench/thesis/REPORT.md + CONTEXT D-02)"; exit 0; \
	  else echo "thesis: hard error rc=$$rc"; exit $$rc; fi
	@echo "thesis: report written; see bench/thesis/REPORT.md"

smoke:
	$(BENCH) $(GO) run ./smoke

compose-smoke: bench/compose-smoke/readiness/bin/readiness
	@echo "compose-smoke: exercising full stack healthcheck gating..."
	$(BENCH) $(GO) test -tags=integration -timeout=35m ./compose-smoke/...

verify: test-unit
	@echo "verify: running full Phase 1 suite (thesis + smoke + compose-smoke)"
	$(MAKE) thesis
	$(MAKE) compose-smoke
	$(MAKE) smoke

test-unit:
	$(BENCH) $(GO) test -short ./...

# -----------------------------------------------------------------------------
# Readiness probe — static Linux binary bind-mounted into the indexd
# container as its healthcheck. See RESEARCH §5.
# -----------------------------------------------------------------------------

build-readiness-probe: bench/compose-smoke/readiness/bin/readiness

bench/compose-smoke/readiness/bin/readiness: bench/compose-smoke/readiness/main.go
	@mkdir -p bench/compose-smoke/readiness/bin
	$(BENCH) GOOS=linux GOARCH=amd64 CGO_ENABLED=0 $(GO) build \
		-tags netgo -ldflags '-s -w' \
		-o compose-smoke/readiness/bin/readiness \
		./compose-smoke/readiness

# -----------------------------------------------------------------------------
# bootstrap-reset — remove wizard-generated local state for a clean re-bootstrap.
# Keeps Compose volumes intact (xorbs stay pinned on Sia; run `make clean` for
# a truly destructive tear-down).
# -----------------------------------------------------------------------------

bootstrap-reset:
	rm -f .env ops/indexd.yml
	@echo "bootstrap-reset: .env + ops/indexd.yml removed; run 'make bootstrap' to redo."

# -----------------------------------------------------------------------------
# A3 Verification — resolves RESEARCH §3 Assumption A3 before PLAN 07 implements
# the bootstrap wizard. Writes .planning/phases/01-validation-foundations/01-A3-VERIFICATION.md.
# -----------------------------------------------------------------------------

a3-verify: bench/compose-smoke/readiness/bin/readiness
	$(COMPOSE) up -d postgres indexd redis
	@echo "a3-verify: waiting for indexd readiness..."
	@until $(COMPOSE) ps --format json | grep -q '"Health":"healthy"'; do sleep 10; echo "  waiting..."; done
	@echo "a3-verify: running probe (timeout 30s for approval)..."
	$(BENCH) $(GO) run -tags=a3probe ./bootstrap
	@echo "a3-verify: see .planning/phases/01-validation-foundations/01-A3-VERIFICATION.md"

# -----------------------------------------------------------------------------
# Destructive
# -----------------------------------------------------------------------------

clean:
	$(COMPOSE) down -v
	rm -rf bench/*/runs bench/compose-smoke/readiness/bin ops/indexd.yml
