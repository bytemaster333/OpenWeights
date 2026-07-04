---
title: API
description: Endpoints exposed by openweights-cas
---

OpenWeights speaks the subset of the Hugging Face Hub API that the `hf`
CLI + `hf_xet` actually hit. If you stick to those clients, you never
need to touch the API directly.

## Auth

Every write endpoint accepts an `Authorization: Bearer <key>` header.
Three valid shapes:

- **OpenWeights API key** (opaque string from `/keys` — SHA-256'd server-side)
- **OpenWeights-minted Xet JWT** (short-lived, HS256, `iss=hf.openweights.app`)
- **Hugging Face Xet JWT**, when traffic arrives through the `hf-proxy`
  shim (`iss=huggingface.co`)

Read endpoints on public repos accept anonymous callers.

## Endpoints

### Hub API

| Method | Path | Notes |
|---|---|---|
| `GET` | `/api/whoami-v2` | Returns the caller's profile |
| `POST` | `/api/repos/create` | Idempotent repo create |
| `POST` | `/api/models/{owner}/{repo}/preupload/{ref}` | Classifies files as `lfs` vs `regular` |
| `GET` | `/api/models/{owner}/{repo}/xet-write-token/{ref}` | Mints a write JWT + CAS URL |
| `GET` | `/api/models/{owner}/{repo}/xet-read-token/{ref}` | Mints a read JWT + CAS URL |
| `POST` | `/{owner}/{repo}.git/info/lfs/objects/batch` | Classic Git-LFS batch |
| `POST` | `/api/models/{owner}/{repo}/commit/{ref}` | Commit the manifest |
| `POST` | `/api/validate-yaml` | Permissive; always returns OK |

### Catalog + resolve

| Method | Path | Notes |
|---|---|---|
| `GET` | `/api/models` | Public repo list |
| `GET` | `/api/models/{owner}/{repo}` | Model info + download counters |
| `GET` | `/api/models/{owner}/{repo}/revision/{rev}` | Same but by SHA |
| `GET` | `/api/models/{owner}/{repo}/downloads/trend` | Zero-filled 14-day trend |
| `GET` \| `HEAD` | `/{owner}/{repo}/resolve/{rev}/{*path}` | Redirects to a byte URL |
| `GET` | `/xet/files/{hash}` | Xorb body (decompressed chunks) |
| `GET` \| `PUT` | `/lfs/objects/{oid}` | Inline LFS content |

### Xet protocol

| Method | Path | Notes |
|---|---|---|
| `POST` | `/v1/xorbs/{prefix}/{hash}` | Xorb upload |
| `POST` | `/shards` + `/v1/shards` | Shard upload |
| `GET` | `/v1/reconstructions/{file_id}` | V1 reconstruction |
| `GET` | `/v2/reconstructions/{file_id}` | V2 (behind a feature flag) |

### Admin / console

The `/admin/*` endpoints back the browser console and require a session
cookie. Not part of the public API surface.
