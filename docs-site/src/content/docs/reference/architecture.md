---
title: Architecture
description: What runs where
---

A SiaHub deployment is five long-running services behind a reverse
proxy, plus Postgres and Redis for state. All services live on the
same Docker Compose network.

## Request flow

An upload with `hf upload` hits `cas`, which classifies the payload
(inline LFS vs xet-chunk), writes metadata to Postgres, and caches
large buffers in `xorb_bodies` while an async worker pins them to Sia
via `indexd`.

A download hits `cas` first (resolve + repo lookup), which issues a
signed URL pointing at `gateway`. The client follows the redirect to
`gateway`, which verifies the signature, range-fetches from Sia
hosts, and streams bytes back.

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
| `users`, `api_keys`, `sessions`, `oauth_state` | Auth state |
| `repos`, `repo_refs`, `repo_commits`, `repo_files` | Model catalog |
| `xorbs`, `shards` | Xet-protocol objects |
| `xorb_bodies` | Local cache of xorb bytes (until pinned) |
| `lfs_objects` | Inline small-file LFS content |
| `repo_downloads` | Per-repo daily download counters |
| `usage_log` | Append-only event log |
| `reconstruction_files`, `reconstruction_terms` | Xet reconstruction manifests |
