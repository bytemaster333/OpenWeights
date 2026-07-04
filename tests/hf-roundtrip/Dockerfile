#HF round-trip harness image.
# Minimal Python image with huggingface-cli + hf (both ship in
# huggingface_hub >= 0.25 when built with the [cli,hf_xet] extras). The
# container is a one-shot runner: the nightly CI job builds it, runs it
# with CAS / gateway URLs + a fresh API key in env, and tears it down.

FROM python:3.12-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
      curl ca-certificates coreutils \
 && rm -rf /var/lib/apt/lists/*

# hf_xet extra pulls in the xet-core Rust core via the python binding — the
# very client Hugging Face ships to HF users. HF_XET_DATA_DEFAULT_CAS_ENDPOINT
# routes its xorb/shard traffic to the OpenWeights CAS under test.
RUN pip install --no-cache-dir "huggingface_hub[cli,hf_xet]>=0.25,<1.0"

COPY run.sh /usr/local/bin/run.sh
RUN chmod +x /usr/local/bin/run.sh

WORKDIR /work
ENTRYPOINT ["/usr/local/bin/run.sh"]
