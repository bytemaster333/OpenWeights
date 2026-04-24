---
title: cli env vars
description: hf CLI env vars that matter for siahub
---

## the two you actually need

```bash
HF_ENDPOINT=https://cas.siahub.app   # route the CLI to a siahub
HF_TOKEN=<your-siahub-key>           # auth for writes + private reads
```

prefix them per-command to avoid polluting your shell config:

```bash
HF_TOKEN=... HF_ENDPOINT=... hf upload <owner>/<repo> ./files
```

## running both siahub + huggingface.co

the per-command prefix is already this pattern — `hf auth login` for
huggingface.co stays put, and siahub commands override with `HF_TOKEN`.
if you want a shorter form:

```fish
function hfsia
  env HF_ENDPOINT=https://cas.siahub.app HF_HOME=$HOME/.cache/huggingface-siahub hf $argv
end
hfsia auth login --token <your-siahub-key>   # once
hfsia upload / download ...                  # thereafter
```

separate `HF_HOME` keeps the two token caches apart.

## less-common vars

- `HF_HUB_DISABLE_TELEMETRY=1` — recommended; no analytics to hf.co
- `HF_DEBUG=1` — verbose request traces (good when a command errors
  and you need the full stack)
- `HF_HUB_VERBOSITY=debug` — log-level knob on the python side
