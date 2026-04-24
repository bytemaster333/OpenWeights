---
title: download a model
description: pull files from a siahub deployment
---

## with the hf CLI

public repos don't need a token:

```bash
HF_ENDPOINT=https://cas.siahub.app hf download <owner>/<repo>
```

by default files land in the standard hf cache
(`~/.cache/huggingface/hub/...`). for a direct dump into the current
directory:

```bash
HF_ENDPOINT=https://cas.siahub.app hf download <owner>/<repo> --local-dir .
```

## single file

```bash
HF_ENDPOINT=https://cas.siahub.app \
  hf download <owner>/<repo> model.safetensors --local-dir ./m
```

## with python

```python
import os
os.environ["HF_ENDPOINT"] = "https://cas.siahub.app"

from huggingface_hub import snapshot_download
path = snapshot_download("<owner>/<repo>")
print(path)
```

## private repos

same as upload — prefix the command with `HF_TOKEN=<your-key>`. the key
must have `download` scope (or `upload`, which implies download).
