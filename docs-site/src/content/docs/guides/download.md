---
title: Download a model
description: Pull files from a SiaHub deployment
---

## With the `hf` CLI

Public repos don't need a token:

```bash
HF_ENDPOINT=https://siahub.app hf download <owner>/<repo>
```

By default files land in the standard `hf` cache
(`~/.cache/huggingface/hub/...`). For a direct dump into the current
directory:

```bash
HF_ENDPOINT=https://siahub.app hf download <owner>/<repo> --local-dir .
```

## Single file

```bash
HF_ENDPOINT=https://siahub.app \
  hf download <owner>/<repo> model.safetensors --local-dir ./m
```

## With Python

```python
import os
os.environ["HF_ENDPOINT"] = "https://siahub.app"

from huggingface_hub import snapshot_download
path = snapshot_download("<owner>/<repo>")
print(path)
```

## Private repos

Same as upload — prefix the command with `HF_TOKEN=<your-key>`. The key
must have `download` scope (or `upload`, which implies download).
