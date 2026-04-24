---
title: Upload a model
description: Push files to a SiaHub deployment
---

## With the `hf` CLI

```bash
HF_TOKEN=<your-key> HF_ENDPOINT=https://siahub.app \
  hf upload <owner>/<repo> ./path/to/files
```

This works for single files (`./model.safetensors`) or directories
(`./my-model/`). Recursion is on by default.

## Filtering

Upload only the weights:

```bash
HF_TOKEN=<your-key> HF_ENDPOINT=https://siahub.app \
  hf upload <owner>/<repo> . --include="*.safetensors"
```

Multiple patterns are supported (`--include="*.json" --include="*.md"`).

## What gets stored where

- **Small text files** (README, `config.json`, `tokenizer.json`) land as
  inline LFS in Postgres — shown once on the model page, cheap to fetch.
- **Large binaries** go through the Xet chunk pipeline: `hf_xet` chunks and
  compresses them, SiaHub receives xorbs, caches them locally, and queues
  a Sia pin. The Sia upload runs asynchronously; the `hf upload` command
  returns as soon as SiaHub has durable bytes.

## Authentication

`HF_TOKEN` is your SiaHub API key, not your huggingface.co token. They
can coexist — your `hf auth login` for huggingface.co keeps working. The
per-command prefix (`HF_TOKEN=... HF_ENDPOINT=...`) only applies to that
invocation.
