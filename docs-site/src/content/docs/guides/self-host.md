---
title: self-host
description: run the full stack on your own box
---

## requirements

- docker + docker compose v2
- 4 GB RAM minimum (indexd alone takes ~1 GB)
- a funded sia wallet — you need storage contracts before pins land
- a domain + tls terminator if you want public access

## clone + configure

```bash
git clone <your-fork> siahub
cd siahub
cp ops/.env.example .env
$EDITOR .env
```

required env vars:

| var | note |
|---|---|
| `SIAHUB_POSTGRES_PASSWORD` | any long random string |
| `REDIS_PASSWORD` | any long random string |
| `GATEWAY_URL_SIGNING_KEY` | base64(32 random bytes) |
| `XET_JWT_SIGNING_KEY` | base64(32 random bytes) |
| `SIAHUB_APP_ID` / `SIAHUB_APP_KEY` | from `make bootstrap` (below) |
| `INDEXD_ADMIN_PASSWORD` | any long random string |
| `GITHUB_OAUTH_CLIENT_ID` + `_SECRET` | from your github oauth app |
| `CAS_PUBLIC_URL` | the url clients reach — `http://localhost:28080` for local |

## bootstrap a sia app key

first boot only:

```bash
make bootstrap    # writes SIAHUB_APP_ID + SIAHUB_APP_KEY to .env
```

this prompts indexd for a fresh app key and prints the recovery phrase.
**do not lose the phrase** — it's the only way to re-derive the same
key if you ever need to migrate. siahub operators keep the phrase in
the `.env`; losing it orphans all stored bytes.

## run

```bash
docker compose -f ops/docker-compose.yml --env-file .env up -d
```

services come up in ~30 s; indexd takes longer on first sync (several
minutes while it pulls the chain tip). the console at `localhost:23000`
stays in "Loading..." until `/admin/me` returns.

## tls + custom domain

caddy config template lives in `ops/Caddyfile`. point your domain at
the box and run caddy with that config; it'll handle acme
automatically. for nginx-based setups, copy the upstreams from the
compose file and front them however you prefer.
