---
title: Self-host
description: Run the full stack on your own box
---

## Requirements

- Docker + Docker Compose v2
- 4 GB RAM minimum (indexd alone takes ~1 GB)
- A funded Sia wallet — you need storage contracts before pins land
- A domain + TLS terminator if you want public access

## Clone + configure

```bash
git clone <your-fork> siahub
cd siahub
cp ops/.env.example .env
$EDITOR .env
```

Required env vars:

| Var | Note |
|---|---|
| `SIAHUB_POSTGRES_PASSWORD` | Any long random string |
| `REDIS_PASSWORD` | Any long random string |
| `GATEWAY_URL_SIGNING_KEY` | `base64(32 random bytes)` |
| `XET_JWT_SIGNING_KEY` | `base64(32 random bytes)` |
| `SIAHUB_APP_ID` / `SIAHUB_APP_KEY` | From `make bootstrap` (below) |
| `INDEXD_ADMIN_PASSWORD` | Any long random string |
| `GITHUB_OAUTH_CLIENT_ID` + `_SECRET` | From your GitHub OAuth app |
| `CAS_PUBLIC_URL` | The URL clients reach — `http://localhost:8080` for local |

## Bootstrap a Sia app key

First boot only:

```bash
make bootstrap    # writes SIAHUB_APP_ID + SIAHUB_APP_KEY to .env
```

This prompts indexd for a fresh app key and prints the recovery phrase.
**Do not lose the phrase** — it's the only way to re-derive the same
key if you ever need to migrate. SiaHub operators keep the phrase in
the `.env`; losing it orphans all stored bytes.

## Run

```bash
docker compose -f ops/docker-compose.yml --env-file .env up -d
```

Services come up in ~30 s; indexd takes longer on first sync (several
minutes while it pulls the chain tip). The console at `localhost:5173`
stays in "Loading..." until `/admin/me` returns.

## TLS + custom domain

A Caddy config template lives in `ops/Caddyfile`. Point your domain at
the box and run Caddy with that config; it will handle ACME
automatically. For nginx-based setups, copy the upstreams from the
compose file and front them however you prefer.
