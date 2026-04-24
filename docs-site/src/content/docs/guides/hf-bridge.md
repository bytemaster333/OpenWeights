---
title: mirror on huggingface
description: pointer-only announce so your model shows up in hf search
---

## why

siahub and huggingface.co aren't exclusive. you can publish bytes to
siahub (where they live on sia) and mirror a pointer to huggingface.co
for discovery — model card, tags, author profile, search ranking —
without sending weights to hf.

## how

open the model detail page at `/models/<owner>/<repo>` while signed
in. an "announce on huggingface.co" card shows up for the owner. pick
your shell (bash / zsh / fish) and copy the command.

the command:

1. writes a short pointer README to your cwd (yaml frontmatter with
   tags, "weights on siahub" body, a `hf download` example)
2. creates the repo on huggingface.co (idempotent — `--exist-ok`)
3. pushes **only** the README

no binary bytes are sent to hf. `hf auth login` for huggingface.co must
already be done on your machine.

## result

- **on huggingface.co**: the repo renders its model card, shows up in
  author profile + search, no download button. visitors who follow the
  README instructions hit siahub for the weights.
- **on siahub**: unchanged — your upload stays authoritative.

## caveats

- hf's "use in transformers" button won't work — there are no weights
  on hf to back it. that's the tradeoff.
- if you rename or unlist on siahub, update the hf pointer README
  manually; the bridge is one-shot.
