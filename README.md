# openweights

xet-compatible model hub backed by sia storage. the standard `hf` CLI
works unmodified — set `HF_ENDPOINT` at a openweights deployment and upload
or download as usual. bytes land on the decentralized sia network
instead of s3.

## run locally

```bash
cp ops/.env.example .env   # fill in the blanks
docker compose -f ops/docker-compose.yml --env-file .env up -d
```

services:

| component | purpose |
|---|---|
| `openweights-cas` | xet protocol + hf-api-compat endpoints (rust, axum) |
| `openweights-gateway` | signed-url byte serving + sia range fetch (go) |
| `openweights-console` | web ui for keys, models, assets, stats (react) |
| `indexd` | sia chain indexer (siafoundation/indexd) |
| `postgres` / `redis` | metadata + rate limits |

console: `http://localhost:5173`
api: `http://localhost:8080`
docs: [docs.openweights.app](https://docs.openweights.app)

## upload / download

mint an api key on the console `/keys` page, then:

```bash
# upload
HF_TOKEN=<your-key> HF_ENDPOINT=http://localhost:8080 \
  hf upload <owner>/<repo> ./files

# download (public repo — no token needed)
HF_ENDPOINT=http://localhost:8080 hf download <owner>/<repo>
```

## license

apache-2.0.
