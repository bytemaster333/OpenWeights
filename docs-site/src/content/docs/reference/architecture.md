---
title: architecture
description: what runs where
---

## components

```
     hf CLI
        |
        v
  +-----------+      +-----------+     +----------+
  |  console  |      |    cas    |---->|  sia     |
  |  (react)  |----->|  (rust)   |     | (indexd) |
  +-----------+      +-----------+     +----------+
                          |    ^
                          v    |
                     +-----------+
                     |  gateway  |
                     |   (go)    |
                     +-----------+
                          |
                          v
                     +-----------+
                     | sia hosts |
                     +-----------+
```

## cas (rust, axum)

the hub + xet protocol server. speaks the hf api surface the `hf`
CLI uses (repos, preupload, commit, resolve, xet write/read tokens)
plus the xet-core wire protocol (xorb upload, shard upload,
reconstruction). metadata lives in postgres; large byte buffers
cache in `xorb_bodies` until the sia pin lands.

## gateway (go)

serves downloads. the signed-url flow mints a short-lived url on
the cas, which the client redeems on the gateway. the gateway
range-fetches from sia hosts and streams bytes back.

## console (react)

browser ui: model catalog, asset inventory, key management, usage
stats, storage provider map. talks to `/admin/*` on the cas.

## hf-proxy (go)

optional. sits in front of huggingface.co and rewrites the
`X-Xet-Cas-Url` header so `hf upload` traffic against hf.co actually
targets a siahub cas. useful when pointing the standard cli at
siahub without changing `HF_ENDPOINT`.

## indexd (sia foundation)

follows the sia chain, maintains the host pool, mediates storage
contracts. siahub talks to its admin api for host geolocation
(`/admin/stats/map`) and to its sdk for byte upload/pin/fetch.

## storage tables

| table | holds |
|---|---|
| `users`, `api_keys`, `sessions` | auth state |
| `repos`, `repo_refs`, `repo_commits`, `repo_files` | model catalog |
| `xorbs`, `shards` | xet-protocol objects |
| `xorb_bodies` | local cache of xorb bytes (until pinned) |
| `lfs_objects` | inline small-file LFS content |
| `repo_downloads` | per-repo daily download counters |
| `usage_log` | append-only event log |
