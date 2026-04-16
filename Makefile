# SiaHub root Makefile.
# Single entry point for every operator task (CONTEXT D-03 + D-07).
# Targets whose recipes depend on code from later plans emit a clear
# "not-yet-implemented" message and exit 2 (distinct from failure exit 1).

.PHONY: bootstrap up down thesis smoke compose-smoke verify test-unit clean \
        build-readiness-probe help

GO      := go
COMPOSE := docker compose -f ops/docker-compose.yml
BENCH   := cd bench &&

# -----------------------------------------------------------------------------
# High-level operator entry points
# -----------------------------------------------------------------------------

help:
	@echo "SiaHub Makefile targets:"
	@echo "  make bootstrap         From-zero-to-running wizard (first-time setup)"
	@echo "  make up                docker compose up -d"
	@echo "  make down              docker compose down"
	@echo "  make thesis            Sia range-download thesis measurement (Phase 1 gate)"
	@echo "  make smoke             1 MiB round-trip test against live indexd"
	@echo "  make compose-smoke     Verify Compose healthchecks pass end-to-end"
	@echo "  make verify            Full suite: unit + thesis + smoke + compose-smoke"
	@echo "  make test-unit         Fast Go unit tests (no network)"
	@echo "  make clean             Tear down Compose + remove volumes (DESTRUCTIVE)"

bootstrap: ops/indexd.yml bench/compose-smoke/readiness/bin/readiness
	@echo "bootstrap: running wizard..."
	$(BENCH) $(GO) run ./bootstrap

up:
	$(COMPOSE) up -d

down:
	$(COMPOSE) down

# -----------------------------------------------------------------------------
# Phase 1 measurement targets
# -----------------------------------------------------------------------------

thesis: bench/compose-smoke/readiness/bin/readiness
	$(COMPOSE) up -d postgres indexd redis
	$(BENCH) $(GO) run ./thesis
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
# indexd.yml template rendering — PLAN 07 provides the real generator
# -----------------------------------------------------------------------------

ops/indexd.yml: ops/indexd.yml.tmpl
	@if [ ! -f ops/indexd.yml.tmpl ]; then \
		echo "ops/indexd.yml.tmpl missing — PLAN 03 provides it"; exit 2; \
	fi
	@echo "ops/indexd.yml generation: PLAN 07 fleshes this target; for now touching placeholder"
	@touch ops/indexd.yml

# -----------------------------------------------------------------------------
# Destructive
# -----------------------------------------------------------------------------

clean:
	$(COMPOSE) down -v
	rm -rf bench/*/runs bench/compose-smoke/readiness/bin ops/indexd.yml
