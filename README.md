# OpenWeights

A Hugging Face–compatible model hub that stores your bytes on the [Sia](https://sia.tech)
decentralized network instead of S3. Point the standard `hf` CLI at an
OpenWeights deployment and upload or download as usual — the files land on Sia
and come back byte-identical. No fork of `huggingface_hub`, no patched `hf_xet`.

📖 **[Documentation](https://bytemaster333.github.io/OpenWeights)**

## How it fits together

| Component | Purpose |
|---|---|
| `openweights-cas` | Xet protocol + HF-API-compat endpoints; writes xorbs to Sia and mints signed gateway URLs (Rust) |
| `openweights-gateway` | Verifies signed URLs and serves byte ranges from Sia, backed by a whole-xorb disk cache (Go) |
| `openweights-console` | Web console for sign-in, API keys, models, assets, and stats (React) |
| `postgres` / `redis` | Metadata / rate limits and request coalescing |

OpenWeights does not bundle a Sia indexer — you point it at one: the hosted
[`https://sia.storage`](https://sia.storage) (a free tier, no wallet funding) or
your own `indexd`. The CAS is the only writer; the gateway is read-only and
serves bytes only for CAS-signed URLs.

## Quickstart

```bash
cp ops/.env.example .env
make setup       # interactive wizard: pick an indexer, set an admin password,
                 # generate every secret → writes .env
make bootstrap   # register the app on your indexer (approve in the browser),
                 # then bring the whole stack up
```

Sign in to the console at `http://localhost:5173`, mint a **read + write** API
key, and use the stock `hf` CLI:

```bash
HF_TOKEN=<key> HF_ENDPOINT=http://localhost:8080 hf upload  <owner>/<repo> ./model-dir
HF_TOKEN=<key> HF_ENDPOINT=http://localhost:8080 hf download <owner>/<repo> --local-dir ./out
```

Upload, download from a clean cache, and compare `sha256sum` — the bytes are
identical. A full round-trip is one command:

```bash
OPENWEIGHTS_API_KEY=<key> bash tests/hf-roundtrip/standalone-roundtrip.sh
```

> **Keep `.env` safe.** `OPENWEIGHTS_RECOVERY_PHRASE` derives your Sia App Key —
> losing it orphans every stored byte permanently.

## Documentation

- **[Get started](https://bytemaster333.github.io/OpenWeights/docs/users/self-host)** —
  self-host for yourself, operate for a team, or use an existing deployment
- **[How it works](https://bytemaster333.github.io/OpenWeights/docs/developers/architecture)** —
  architecture, the Xet protocol implementation, and the API reference

## Prebuilt images

The service images publish to `ghcr.io/bytemaster333/openweights-*` on each
release. To run from them instead of building locally:

```bash
docker compose -f ops/docker-compose.yml pull
make up
```

## License

[Apache-2.0](LICENSE).
