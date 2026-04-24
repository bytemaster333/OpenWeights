---
title: quickstart
description: upload and download your first model in three commands
---

## 1. get an api key

open the console at `/keys`, click **create key**, copy the plaintext
value shown in the modal. it's the only time you'll see it.

## 2. upload

```bash
HF_TOKEN=<your-key> HF_ENDPOINT=https://cas.siahub.app \
  hf upload <owner>/<repo> ./files
```

replace `<owner>` with your github login. `<repo>` can be anything that
matches `[a-z0-9._-]{1,96}`.

## 3. download

no token needed for public repos:

```bash
HF_ENDPOINT=https://cas.siahub.app hf download <owner>/<repo>
```

or drop the `HF_ENDPOINT` override on a siahub.app domain and it will
default to the hosted deployment.

## what happened

the `hf` CLI talked to siahub exactly the way it talks to
huggingface.co. siahub wrote the bytes to a local cache, queued a pin
to the sia network, and stamped a commit on your repo. the download
path pulled bytes back from the local cache (and, once the pin lands,
directly from sia hosts).
