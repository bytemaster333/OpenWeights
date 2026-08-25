# openweights

xet-compatible model hub backed by sia storage. the standard `hf` CLI works
unmodified — point `HF_ENDPOINT` at an openweights deployment and upload or
download as usual. bytes land on the decentralized sia network instead of s3,
and come back byte-identical.

## how it fits together

| component | purpose |
|---|---|
| `openweights-cas` | xet protocol + hf-api-compat endpoints; writes xorbs to sia, mints signed gateway URLs (rust, axum) |
| `openweights-gateway` | verifies signed URLs, range-fetches + decrypts xorbs from sia, serves bytes (go) |
| `openweights-console` | web ui for sign-in, api keys, models, assets, stats (react) |
| `postgres` / `redis` | metadata + rate limits / request coalescing |

OpenWeights does **not** bundle an indexer — you point it at one (see below).
The cas is the only writer (it holds the sia app key); the gateway is read-only
and serves bytes only for cas-signed URLs. on download, `hf` asks the cas for a
reconstruction, then fetches the xorbs straight from the gateway.

## run it

```bash
cp ops/.env.example .env
make setup            # interactive wizard: pick an indexer, set an admin
                      # password, generate all secrets + keys → writes .env
make bootstrap        # registers the sia app on the indexer (approve in your
                      # browser when prompted), then brings the whole stack up
```

`make setup` fills `.env` (passwords, signing keys, App ID). `make bootstrap`
then runs the app-approval flow and writes `OPENWEIGHTS_APP_KEY`. **Keep `.env`
(and especially `OPENWEIGHTS_RECOVERY_PHRASE`) safe — losing the phrase orphans
every stored byte permanently.**

Already have a fully-populated `.env`? Just `make up`.

The four service images are published to
`ghcr.io/bytemaster333/openweights-{cas,gateway,console,hf-proxy}`. To run from
prebuilt images instead of building locally, pull them first:

```bash
docker compose -f ops/docker-compose.yml pull
make up
```

### choosing an indexer

OpenWeights reaches Sia through an indexer you supply via
`OPENWEIGHTS_INDEXER_URL`. Two options:

- **Hosted (default): `https://sia.storage`** — 50 GB free tier, no wallet
  funding, a deep host pool. `make setup` picks this unless you choose otherwise.
- **Your own `indexd`** — set the URL to your instance
  (`http://my-indexd:9982` or `https://indexd.example.com`). You fund its wallet
  and keep it synced.

## upload / download

Sign in to the console at `http://localhost:5173` (password auth uses
`OPENWEIGHTS_ADMIN_PASSWORD` from `.env`; GitHub OAuth is optional). Mint a key
on the `/keys` page — the default **read + write** scope carries both upload and
download, so one key does a full round-trip. Then use the stock `hf` CLI with
`HF_ENDPOINT` pointed at the cas:

```bash
# upload
HF_TOKEN=<your-key> HF_ENDPOINT=http://localhost:8080 \
  hf upload <owner>/<repo> ./model-dir

# download from a fresh cache (same read+write key, or a read key)
HF_TOKEN=<your-key> HF_ENDPOINT=http://localhost:8080 \
  hf download <owner>/<repo> --local-dir ./out
```

### proving the round-trip

The whole point is a byte-identical round-trip. Upload a file, download it from
a clean cache, and compare hashes:

```bash
sha256sum ./model-dir/weights.bin        # before
sha256sum ./out/weights.bin              # after — identical
```

An end-to-end harness that does exactly this (upload → wait for the async pin →
fresh-cache download → sha compare) lives at
`tests/hf-roundtrip/standalone-roundtrip.sh`.

## notes

- **first pin is slow.** the first upload on a fresh indexer forms on-chain
  storage contracts across many hosts; the upload is accepted immediately and a
  background reconciler finishes the pin. a download before the pin lands
  returns 404 at reconstruction, so retry (the harness does).
- **api** is on `http://localhost:8080`, **console** on `http://localhost:5173`,
  docs at [docs](https://bytemaster333.github.io/OpenWeights).
- everything is driven through the root `Makefile` (`make help` lists targets).
