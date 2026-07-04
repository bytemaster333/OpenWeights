---
title: Mirror on Hugging Face
description: Pointer-only announce so your model shows up in HF search
---

## Why

OpenWeights and huggingface.co aren't exclusive. You can publish bytes to
OpenWeights (where they live on Sia) and mirror a pointer to huggingface.co
for discovery — model card, tags, author profile, search ranking —
without sending weights to HF.

## How

Open the model detail page at `/models/<owner>/<repo>` while signed
in. An "Announce on huggingface.co" card shows up for the owner. Pick
your shell (bash / zsh / fish) and copy the command.

The command:

1. Writes a short pointer README to your cwd (YAML frontmatter with
   tags, a "weights on OpenWeights" body, and an `hf download` example)
2. Creates the repo on huggingface.co (idempotent — `--exist-ok`)
3. Pushes **only** the README

No binary bytes are sent to HF. `hf auth login` for huggingface.co must
already be done on your machine.

## Result

- **On huggingface.co**: the repo renders its model card, shows up in
  author profile + search, with no download button. Visitors who follow
  the README instructions hit OpenWeights for the weights.
- **On OpenWeights**: unchanged — your upload stays authoritative.
