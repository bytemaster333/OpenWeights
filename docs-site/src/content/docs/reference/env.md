---
title: CLI env vars
description: hf CLI env vars that matter for OpenWeights
---

## The two you actually need

```bash
HF_ENDPOINT=https://openweights.app       # route the CLI to OpenWeights
HF_TOKEN=<your-openweights-key>           # auth for writes + private reads
```

Prefix them per-command to avoid polluting your shell config:

```bash
HF_TOKEN=... HF_ENDPOINT=... hf upload <owner>/<repo> ./files
```

## Running both OpenWeights + huggingface.co

The per-command prefix is already this pattern — `hf auth login` for
huggingface.co stays put, and OpenWeights commands override with `HF_TOKEN`.
If you want a shorter form:

```fish
function hfsia
  env HF_ENDPOINT=https://openweights.app HF_HOME=$HOME/.cache/huggingface-openweights hf $argv
end
hfsia auth login --token <your-openweights-key>   # once
hfsia upload / download ...                  # thereafter
```

A separate `HF_HOME` keeps the two token caches apart.

## Less-common vars

- `HF_HUB_DISABLE_TELEMETRY=1` — recommended; no analytics to hf.co
- `HF_DEBUG=1` — verbose request traces (useful when a command errors
  and you need the full stack)
- `HF_HUB_VERBOSITY=debug` — log-level knob on the Python side
