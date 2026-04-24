---
title: Architecture
description: What runs where
---

## Components

```
     hf CLI
        |
        v
  +-----------+      +-----------+     +----------+
  |  console  |      |    cas    |---->|   Sia    |
  |  (React)  |----->|  (Rust)   |     | (indexd) |
  +-----------+      +-----------+     +----------+
                          |    ^
                          v    |
                     +-----------+
                     |  gateway  |
                     |   (Go)    |
                     +-----------+
                          |
                          v
                     +-----------+
                     | Sia hosts |
                     +-----------+
```

## cas (Rust, Axum)

The hub + Xet protocol server. Speaks the HF API surface the `hf` CLI
uses (repos, preupload, commit, resolve, xet write/read tokens) plus
the xet-core wire protocol (xorb upload, shard upload, reconstruction).
Metadata lives in Postgres; large byte buffers cache in `xorb_bodies`
until the Sia pin lands.

## gateway (Go)

Serves downloads. The signed-URL flow mints a short-lived URL on the
CAS, which the client redeems on the gateway. The gateway range-fetches
from Sia hosts and streams bytes back.

## console (React)

Browser UI: model catalog, asset inventory, key management, usage
stats, storage provider map. Talks to `/admin/*` on the CAS.

## hf-proxy (Go)

Optional. Sits in front of huggingface.co and rewrites the
`X-Xet-Cas-Url` header so `hf upload` traffic against hf.co actually
targets a SiaHub CAS. Useful when pointing the standard CLI at SiaHub
without changing `HF_ENDPOINT`.

## indexd (Sia Foundation)

Follows the Sia chain, maintains the host pool, mediates storage
contracts. SiaHub talks to its admin API for host geolocation
(`/admin/stats/map`) and to its SDK for byte upload/pin/fetch.

## Storage tables

| Table | Holds |
|---|---|
| `users`, `api_keys`, `sessions` | Auth state |
| `repos`, `repo_refs`, `repo_commits`, `repo_files` | Model catalog |
| `xorbs`, `shards` | Xet-protocol objects |
| `xorb_bodies` | Local cache of xorb bytes (until pinned) |
| `lfs_objects` | Inline small-file LFS content |
| `repo_downloads` | Per-repo daily download counters |
| `usage_log` | Append-only event log |
