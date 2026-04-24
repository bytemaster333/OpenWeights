---
title: api
description: endpoints exposed by siahub-cas
---

siahub speaks the subset of the hugging face hub api that the `hf`
CLI + `hf_xet` actually hit. if you stick to those clients, you never
need to touch the api directly.

## auth

every write endpoint accepts an `Authorization: Bearer <key>` header.
three valid shapes:

- siahub api key (opaque string from `/keys` — sha-256'd server-side)
- siahub-minted xet jwt (short-lived, hs256, `iss=hf.siahub.app`)
- hugging face xet jwt, when traffic arrives through the `hf-proxy`
  shim (`iss=huggingface.co`)

read endpoints on public repos accept anonymous callers.

## endpoints

### hub api

| method | path | notes |
|---|---|---|
| `GET` | `/api/whoami-v2` | returns the caller's profile |
| `POST` | `/api/repos/create` | idempotent repo create |
| `POST` | `/api/models/{owner}/{repo}/preupload/{ref}` | classifies files as `lfs` vs `regular` |
| `GET` | `/api/models/{owner}/{repo}/xet-write-token/{ref}` | mints a write jwt + cas url |
| `GET` | `/api/models/{owner}/{repo}/xet-read-token/{ref}` | mints a read jwt + cas url |
| `POST` | `/{owner}/{repo}.git/info/lfs/objects/batch` | classic git-lfs batch |
| `POST` | `/api/models/{owner}/{repo}/commit/{ref}` | commit the manifest |
| `POST` | `/api/validate-yaml` | permissive; always returns ok |

### catalog + resolve

| method | path | notes |
|---|---|---|
| `GET` | `/api/models` | public repo list |
| `GET` | `/api/models/{owner}/{repo}` | model info + download counters |
| `GET` | `/api/models/{owner}/{repo}/revision/{rev}` | same but by sha |
| `GET` | `/api/models/{owner}/{repo}/downloads/trend` | zero-filled 14-day trend |
| `GET` \| `HEAD` | `/{owner}/{repo}/resolve/{rev}/{*path}` | redirects to a byte url |
| `GET` | `/xet/files/{hash}` | xorb body (decompressed chunks) |
| `GET` \| `PUT` | `/lfs/objects/{oid}` | inline lfs content |

### xet protocol

| method | path | notes |
|---|---|---|
| `POST` | `/v1/xorbs/{prefix}/{hash}` | xorb upload |
| `POST` | `/shards` + `/v1/shards` | shard upload |
| `GET` | `/v1/reconstructions/{file_id}` | v1 reconstruction |
| `GET` | `/v2/reconstructions/{file_id}` | v2 (behind a feature flag) |

### admin / console

`/admin/*` endpoints back the browser console and require a session
cookie. not part of the public api surface.
