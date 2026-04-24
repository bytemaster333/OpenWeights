---
title: Quickstart
description: Upload and download your first model in three commands
---

## 1. Get an API key

Open the console at `/keys`, click **Create key**, and copy the plaintext
value shown in the modal. It's the only time you'll see it.

## 2. Upload

```bash
HF_TOKEN=<your-key> HF_ENDPOINT=https://siahub.app \
  hf upload <owner>/<repo> ./files
```

Replace `<owner>` with your GitHub login. `<repo>` can be anything that
matches `[a-z0-9._-]{1,96}`.

## 3. Download

No token needed for public repos:

```bash
HF_ENDPOINT=https://siahub.app hf download <owner>/<repo>
```

## What happened

The `hf` CLI talked to SiaHub exactly the way it talks to
huggingface.co. SiaHub wrote the bytes to a local cache, queued a pin
to the Sia network, and stamped a commit on your repo. The download
path pulled bytes back from the local cache (and, once the pin lands,
directly from Sia hosts).
