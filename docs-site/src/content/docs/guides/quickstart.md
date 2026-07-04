---
title: Quickstart
description: Upload and download your first model in three commands
---

## 1. Get an API key

Open the console at `/keys`, click **Create key**, and copy the plaintext
value shown in the modal. It's the only time you'll see it.

## 2. Upload

```bash
HF_TOKEN=<your-key> HF_ENDPOINT=https://openweights.app \
  hf upload <owner>/<repo> ./files
```

Replace `<owner>` with your GitHub login. `<repo>` can be any
alphanumeric string (up to 96 chars) containing `-`, `_`, or `.` — it
just can't start with a dot.

## 3. Download

No token needed for public repos:

```bash
HF_ENDPOINT=https://openweights.app hf download <owner>/<repo>
```

## What happened

The `hf` CLI talked to OpenWeights exactly the way it talks to
huggingface.co. OpenWeights wrote the bytes to a local cache, queued a pin
to the Sia network, and stamped a commit on your repo. The download
path pulled bytes back from the local cache (and, once the pin lands,
directly from Sia hosts).
