---
title: upload a model
description: push files to a siahub deployment
---

## with the hf CLI

```bash
HF_TOKEN=<your-key> HF_ENDPOINT=https://cas.siahub.app \
  hf upload <owner>/<repo> ./path/to/files
```

this works for single files (`./model.safetensors`) or directories
(`./my-model/`). recursion is on by default.

## filtering

upload only the weights:

```bash
HF_TOKEN=<your-key> HF_ENDPOINT=https://cas.siahub.app \
  hf upload <owner>/<repo> . --include="*.safetensors"
```

multiple patterns are supported (`--include="*.json" --include="*.md"`).

## what gets stored where

- **small text files** (README, config.json, tokenizer.json) land as
  inline LFS in postgres — shown once on the model page, cheap to fetch.
- **large binaries** go through the xet chunk pipeline: hf_xet chunks +
  compresses them, siahub receives xorbs, caches them locally, queues
  a sia pin. the sia upload runs asynchronously; the `hf upload` command
  returns as soon as siahub has durable bytes.

## authentication

`HF_TOKEN` is your siahub api key, not your huggingface.co token. they
can coexist — your `hf auth login` for huggingface.co keeps working. the
per-command prefix (`HF_TOKEN=... HF_ENDPOINT=...`) only applies to that
invocation.
