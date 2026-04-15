# SiaHub

Third-party, Xet-compatible storage backend on Sia. Self-hostable infrastructure + a single demo instance at `siahub.app` + `cas.siahub.app`.

**Core value:** any Hugging Face user who points `HF_XET_DATA_DEFAULT_CAS_ENDPOINT` at a SiaHub deployment and runs `huggingface-cli upload` / `hf download` gets a byte-identical round-trip with bytes stored on Sia instead of S3.

## Quick start

```sh
cp .env.example .env
# (optionally edit .env — all required fields are auto-generated on first bootstrap if left empty)
make bootstrap
```

`make bootstrap` is a from-zero-to-running wizard: brings up Compose, waits for indexd sync, prompts for wallet funding at the Zen faucet, derives the App Key, and runs the smoke test.

## Repository layout

- `cas/` — `siahub-cas` (Rust/Axum) — Phase 2
- `gateway/` — `siahub-gateway` (Go/chi) — Phase 3
- `console/` — `siahub-console` (Vite/React) — Phase 4
- `bench/` — Go validators (thesis, smoke, bootstrap, compose-smoke) — Phase 1
- `ops/` — Docker Compose, `.env.example`, Caddy config (Phase 6)
- `conformance/` — `siahub-conformance` Rust crate — Phase 2
- `docs/` — benchmarks report + self-hosting guide — Phases 5/6
- `.planning/` — planning artifacts (committed; see `.planning/PROJECT.md`)

## Makefile targets

| Target | Description |
|--------|-------------|
| `make bootstrap` | From-zero-to-running wizard (first-time setup) |
| `make up` / `make down` | Start/stop Compose stack |
| `make thesis` | Run the Sia range-download thesis measurement (Phase 1 gate) |
| `make smoke` | 1 MiB round-trip test against live indexd |
| `make compose-smoke` | Verify Compose healthchecks pass end-to-end |
| `make verify` | Full suite: unit + thesis + smoke + compose-smoke |
| `make test-unit` | Fast Go unit tests (no network) |
| `make clean` | Tear down Compose + remove volumes (DESTRUCTIVE) |

## Documentation

See `.planning/PROJECT.md` for locked scope, `.planning/ROADMAP.md` for the 6-phase build order, and `notes.md` for contributor guidance.
