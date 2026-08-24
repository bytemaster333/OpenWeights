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
| `openweights-console` | web ui for github sign-in, api keys, models, assets, stats (react) |
| `indexd` | sia chain indexer (siafoundation/indexd) |
| `postgres` / `redis` | metadata + rate limits / request coalescing |

the cas is the only writer (it holds the sia app key); the gateway is read-only
and serves bytes only for cas-signed URLs. on download, `hf` asks the cas for a
reconstruction, then fetches the xorbs straight from the gateway.

## run it

```bash
cp ops/.env.example .env
make bootstrap        # first-time wizard: generates secrets, brings the stack
                      # up, funds the wallet, registers the sia app, smoke-tests
# or, if you already have a populated .env:
make up
```

`make bootstrap` fills the blank secrets in `.env` and writes
`OPENWEIGHTS_APP_ID` / `OPENWEIGHTS_APP_KEY` after registering the app with the
indexer. **Keep `.env` (and especially `OPENWEIGHTS_RECOVERY_PHRASE`) safe —
losing the phrase orphans every stored byte permanently.**

### choosing an indexer

The stack defaults to the bundled self-hosted `indexd`, which needs a funded
wallet and enough usable hosts to form storage contracts. If contract formation
stalls (e.g. on a sparse testnet), point everything at a hosted mainnet indexer
with a deep host pool by setting one line in `.env`:

```bash
OPENWEIGHTS_INDEXER_URL=https://sia.storage
```

then re-register the app against it (`make bootstrap` does this) and `make up`.

## upload / download

Sign in to the console (`http://localhost:5173`), mint a key on the `/keys`
page, then use the stock `hf` CLI with `HF_ENDPOINT` pointed at the cas:

```bash
# upload
HF_TOKEN=<your-key> HF_ENDPOINT=http://localhost:8080 \
  hf upload <owner>/<repo> ./model-dir

# download from a fresh cache (public repo needs no token)
HF_ENDPOINT=http://localhost:8080 \
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
  docs at [docs.openweights.app](https://docs.openweights.app).
- everything is driven through the root `Makefile` (`make help` lists targets).
