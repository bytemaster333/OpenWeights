# SiaHub

Third-party, Xet-compatible storage backend on Sia. Any Hugging Face user who points `HF_XET_DATA_DEFAULT_CAS_ENDPOINT` at a SiaHub deployment and runs `huggingface-cli upload` / `hf download` gets a byte-identical round-trip with bytes stored on Sia instead of S3.

This README walks a fresh Linux host from `apt-get install` to a working `siahub.app`-equivalent deployment. Self-hostable infrastructure; one owner-run demo at `siahub.app` + `cas.siahub.app`.

[![Conformance](https://img.shields.io/endpoint?url=https%3A%2F%2Fsiahub.app%2Fconformance-badge.json)](https://siahub.app/conformance-badge.json)

## Prerequisites

- Linux host (`x86_64` or `arm64`; tested on Ubuntu 24.04).
- Docker Engine 25+ and Docker Compose v2.
- A domain you control, with two A-records (`your-domain.tld` and `cas.your-domain.tld`) pointed at the server's public IP.
- Ports 80 and 443 open to the public internet.
- A Siacoin wallet balance: **zSC on Zen testnet** for dev/demo work, or **SC on mainnet** for production deploys. ~1 zSC is enough to form initial storage contracts (threshold is `SIAHUB_WALLET_THRESHOLD_HASTINGS`, default 1 zSC).

Toolchains needed only if you plan to rebuild the service images locally (pre-built images are published):

- Go 1.26 (`siahub-gateway`, `bench/`)
- Rust 1.89 (`siahub-cas`, `conformance/`)
- pnpm 9.x + Node 22 LTS (`siahub-console`)

Install the Docker base toolchain on Ubuntu:

```
sudo apt-get update
sudo apt-get install -y docker.io docker-compose-plugin git curl jq openssl make
```

## Environment Variables

All configuration lives in a single `.env` file at the repo root. The template is `ops/.env.example` (mirrored at the repo-root `.env.example`). Copy it and edit in place:

```
cp ops/.env.example .env
chmod 0600 .env
```

**Never commit `.env`** — it is gitignored. `chmod 0600` keeps the secret store owner-only.

Leaving required fields blank is explicitly supported: `make bootstrap` generates random values on first run and writes them back into `.env` (see §First-run setup).

| Variable | Purpose | Ownership |
|---|---|---|
| `SIAHUB_RECOVERY_PHRASE` | BIP-39 12-word seed for the indexd wallet. | **CRITICAL**: losing this orphans every byte you've stored. Store off-server in a password manager. Inverse of Sia's end-user guidance — see §Destructive commands. Leave blank on first boot to auto-generate. |
| `SIAHUB_APP_ID` | 32-byte hex constant identifying this SiaHub deployment to indexd. | Bootstrap reads from `bench/appid`; operator does not edit. |
| `SIAHUB_APP_KEY` | App key derived from the recovery phrase. | Bootstrap writes after derivation; operator does not edit. |
| `SIAHUB_INDEXER_URL` | Internal URL the CAS uses to reach indexd. | Default `http://indexd:9982` is correct for Compose. |
| `SIA_NETWORK` | `zen` (testnet) or `mainnet`. | Operator-set. Default `zen`. |
| `POSTGRES_SUPERUSER_PASSWORD` | Postgres 17 superuser (shared container, two DBs). | Operator-set, 32+ random bytes. Blank = bootstrap generates. |
| `SIAHUB_POSTGRES_PASSWORD` | `siahub-cas` role on the `siahub` DB. | Operator-set. Blank = bootstrap generates. |
| `SIAHUB_GW_POSTGRES_PASSWORD` | `siahub_gw` read-only role used by the gateway. | Operator-set. Blank = bootstrap generates. |
| `INDEXD_ADMIN_PASSWORD` | indexd admin API password. | Operator-set. Blank = bootstrap generates. |
| `INDEXD_POSTGRES_PASSWORD` | indexd's Postgres role. | Operator-set. Blank = bootstrap generates. |
| `REDIS_PASSWORD` | Redis AUTH password. | Operator-set. Blank = bootstrap generates. |
| `GATEWAY_URL_SIGNING_KEY` | HMAC-SHA256 key (base64 32 bytes) shared by CAS (mints signed URLs) and gateway (verifies). | Operator-set: `openssl rand 32 \| base64`. Blank = bootstrap generates. |
| `GATEWAY_URL_SIGNING_KEY_PREV` | Previous HMAC key during rotation; gateway accepts either. | Operator-set during key rotation only; otherwise empty. |
| `GATEWAY_URL_TTL_SECS` | TTL in seconds for minted gateway URLs. | Default `7200` (2h). |
| `GATEWAY_BASE_URL` | Public URL base stamped into signed URLs. | Production: `https://cas.your-domain.tld`. Local: `http://127.0.0.1:9090`. |
| `GATEWAY_CACHE_DIR` | Whole-xorb disk LRU root inside the gateway container. | Default `/var/cache/siahub`. |
| `GATEWAY_CACHE_SIZE_BYTES` | LRU budget. | Default 100 GiB (`107374182400`); tune to your disk. |
| `V2_RECONSTRUCTION_ENABLED` | Xet-core V2 multi-range reconstruction. | Keep `true` for production; V1 works either way. Set `false` only to debug a V2 regression. |
| `GITHUB_OAUTH_CLIENT_ID` | GitHub OAuth App client ID. | Operator creates the OAuth App (see §Creating a GitHub OAuth App). |
| `GITHUB_OAUTH_CLIENT_SECRET` | GitHub OAuth App client secret. | Operator creates. |
| `GITHUB_OAUTH_CALLBACK_URL` | Must match the callback URL registered on GitHub, exactly. | Operator-set, e.g. `https://cas.your-domain.tld/auth/github/callback`. |
| `CONSOLE_BASE_URL` | Console origin CAS redirects to after OAuth callback. | Operator-set, e.g. `https://your-domain.tld`. |
| `INDEXD_IMAGE` | Pinned indexd image ref. | Default `ghcr.io/siafoundation/indexd:latest`; pin to a digest in production. |
| `RUST_LOG` | `siahub-cas` log level. | Default `info`. |

Random-value helper: `openssl rand -hex 32` (hex) or `openssl rand 32 \| base64` (base64).

## Starting indexd + wallet funding

`indexd` is the Sia Foundation's self-hosted indexer. First boot requires a **10–30 minute consensus sync** before storage contracts can form — do not be alarmed when the stack sits at "starting" for the first half-hour.

Bring up Postgres + Redis + indexd alone to watch the sync:

```
make up                       # equivalent to: docker compose -f ops/docker-compose.yml up -d
docker compose logs -f indexd # wait for consensus-synced log lines
```

The indexd container's healthcheck invokes a bind-mounted readiness probe that blocks on `consensus.synced == true AND wallet.confirmed_balance > SIAHUB_WALLET_THRESHOLD_HASTINGS` (default 1 zSC). The container stays "starting" until **both** conditions are true; there is no silent fallback.

**Fund the wallet.** `make bootstrap` prints the wallet address when it detects `confirmed_balance == 0`. Paste that address into:

- **Zen testnet (dev/demo):** `https://zen.siascan.com/faucet` (manual — requires CAPTCHA). The faucet's `--instant` endpoint is currently broken on Zen; use the web UI only.
- **Mainnet (production):** acquire Siacoin on an exchange and send to the printed address.

Confirmation window: ~1–2 minutes on Zen, ~10–20 minutes on mainnet. Bootstrap resumes automatically once the balance crosses the threshold.

If you need to check wallet state manually:

```
docker compose exec indexd wget -qO- \
  --user=admin --password="$INDEXD_ADMIN_PASSWORD" \
  http://localhost:9980/api/wallet
```

## Creating a GitHub OAuth App

SiaHub uses GitHub OAuth as its sole auth provider. Each self-hoster registers their own OAuth App — there is no shared client.

1. Visit <https://github.com/settings/developers>, click **OAuth Apps** → **New OAuth App**.
2. **Application name:** any label (e.g., `SiaHub — your-domain.tld`).
3. **Homepage URL:** `https://your-domain.tld`.
4. **Authorization callback URL:** `https://cas.your-domain.tld/auth/github/callback` — this must match `GITHUB_OAUTH_CALLBACK_URL` in `.env` character-for-character. Mismatch surfaces as an opaque `redirect_uri_mismatch` from GitHub and is the single most common self-hosting snag.
5. Click **Register application**, then copy the `Client ID` and generate a `Client secret`. Paste both into `.env` as `GITHUB_OAUTH_CLIENT_ID` and `GITHUB_OAUTH_CLIENT_SECRET`.

Canonical GitHub docs: <https://docs.github.com/en/apps/oauth-apps/building-oauth-apps/creating-an-oauth-app>

No screenshots here — GitHub's UI changes often and screenshots rot. The five labels above hold even when the layout shifts.

## First-run setup

Once `.env` is filled in (or left blank for auto-generation) and the OAuth App is registered:

```
make bootstrap
```

`make bootstrap` is a from-zero-to-running wizard: it generates missing secrets, derives the App Key from the recovery phrase, brings up the Compose stack, waits for `indexd` to sync, prompts for wallet funding if needed, and runs a 1 MiB round-trip smoke test.

After it exits clean:

1. Visit `https://your-domain.tld`. You should see the SiaHub landing page.
2. Click **Sign in with GitHub** and complete the OAuth flow.
3. New users land on `/onboarding` — follow the copy-paste card to:
   - Issue your first API key (shown exactly once; copy it immediately).
   - Export `HF_XET_DATA_DEFAULT_CAS_ENDPOINT` and `HF_XET_DATA_CUSTOM_HEADERS` in your shell.
   - Run `huggingface-cli upload` against a small test repo to confirm end-to-end flow.
4. Visit `/setup` (admin-only). Confirm all five diagnostic tiles are green: Postgres, Redis, indexd (synced + funded), GitHub OAuth (configured), V2 reconstruction (enabled).
5. The `<Header>` shows a `Conformance: PASS` badge once the CI workflow has published `conformance-badge.json`; it may read `unknown` on a fresh fork until the first CI run lands.

## Backup & restore

SiaHub's durable state lives in two Postgres databases: `siahub` (users, API keys, xorb metadata, usage log) and `indexd` (Sia host scoring, contract state). **Xorbs themselves are pinned on Sia** and survive local wipes — that IS the disaster-recovery story for the bytes. But the mapping between Merkle hashes and Sia object IDs lives only in Postgres, so back up Postgres periodically.

Two `pg_dump` one-liners — run as cron jobs or on demand:

```
# siahub DB (xorb metadata, users, API keys, usage log)
docker compose exec -T postgres pg_dump -U postgres siahub \
  > siahub-$(date -u +%Y-%m-%d).sql

# indexd DB (Sia host scoring, contract state)
docker compose exec -T postgres pg_dump -U postgres indexd \
  > indexd-$(date -u +%Y-%m-%d).sql
```

Restore onto a fresh host (after `make up` so Postgres is running):

```
# siahub
docker compose exec -T postgres psql -U postgres -c "CREATE DATABASE siahub;"
docker compose exec -T postgres psql -U postgres -d siahub < siahub-YYYY-MM-DD.sql

# indexd
docker compose exec -T postgres psql -U postgres -c "CREATE DATABASE indexd;"
docker compose exec -T postgres psql -U postgres -d indexd < indexd-YYYY-MM-DD.sql
```

Migrations are **idempotent and restart-safe** (all `ADD COLUMN IF NOT EXISTS` / `CREATE TABLE IF NOT EXISTS`). `siahub-cas` can be started onto a restored DB without any manual migration step.

## Destructive commands

Two commands destroy local state. Read before you run them.

> **WARNING — `docker compose down -v`** (and `make clean`, which wraps it) deletes every Compose volume:
> - Both Postgres databases (xorb → Sia-object-ID mapping, users, API keys, usage log, indexd's host scoring + contract state).
> - Redis rate-limit state.
> - The gateway's LRU disk cache.
> - The indexd consensus-sync + wallet data (you will re-sync consensus on next boot — another 10–30 minute window).
>
> **What survives on Sia:** every uploaded xorb remains pinned on Sia hosts for its contract term. The bytes are not lost. But without the Postgres mapping table, SiaHub cannot look up which Sia object corresponds to which xorb hash — they are orphaned from SiaHub's view even though Sia still holds them. The mapping table is technically rebuildable from the shards stored on Sia, but v1 does not ship that recovery path.
>
> **Back up Postgres before running `down -v` or `make clean`.**

### The recovery phrase

`SIAHUB_RECOVERY_PHRASE` in `.env` is the BIP-39 seed for the indexd wallet. **Losing this phrase orphans every byte you have stored.** Sia hosts continue holding the data for the contract term, but no wallet = no contract renewals and no retrieval authority.

This is the inverse of Sia's end-user guidance (which says "discard after onboarding"). SiaHub is infrastructure — the operator is the wallet owner, and the phrase MUST stay in the operator-owned `.env` forever. Mirror it to an off-server password manager and treat `.env` at `chmod 0600` on a single server as the authoritative secret store.

## Benchmarks

A 3-trial median comparison of cold-cache download, warm-cache download, and upload throughput against Hugging Face's native S3+CloudFront backend on the same pinned model lives in `docs/benchmarks.md`. Regenerate with `make bench` (see the benchmarks doc for methodology and fixture pins).

## Architecture

Three services at the repo root — `siahub-cas` (Rust/Axum; Xet protocol), `siahub-gateway` (Go/chi; byte-range serving + disk LRU over `go.sia.tech/siastorage`), and `siahub-console` (Vite/React/shadcn; operator UI) — plus `indexd`, Postgres 17, and Redis 7.4 in the same Compose stack, fronted by Caddy. `siahub-cas` mints HMAC-signed URLs that `siahub-gateway` validates; all Sia I/O is delegated to `go.sia.tech/siastorage` backed by self-hosted `indexd`. Full architecture write-up with request-flow diagrams is at `.planning/research/ARCHITECTURE.md`.

## License / Contributing / Grant context

SiaHub is submitted as infrastructure accompanying a Sia Foundation grant proposal. The planning tree under `.planning/` is committed deliberately so reviewers can see the requirements (`REQUIREMENTS.md`), roadmap (`ROADMAP.md`), phase-by-phase plans (`.planning/phases/`), and research synthesis (`.planning/research/`). Start with `.planning/PROJECT.md` for locked scope and `notes.md` for contributor guidance.

License and contribution guide land at grant-submission time; until then, this is a single-owner build.
