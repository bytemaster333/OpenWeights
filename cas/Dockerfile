# syntax=docker/dockerfile:1.7
FROM rust:1.85-bookworm AS builder
WORKDIR /src

# Copy workspace manifests first for cached dep layer. Cargo.lock is
# REQUIRED here (with --locked below): without it cargo re-resolves the whole
# graph from scratch inside the image and can pick a transitive version the
# pinned crates don't compile against (e.g. xet-runtime newer than the
# xet-core-structures pin expects). The committed lock is the reproducible graph.
COPY cas/Cargo.toml cas/Cargo.lock cas/rust-toolchain.toml ./cas/
COPY cas/crates ./cas/crates
# Vendored sia_storage 0.7.0 (one-constant patch — see cas/Cargo.toml
# [patch.crates-io]).
COPY cas/vendored ./cas/vendored

# migrations/ MUST be present at build time so sqlx::migrate!("../../migrations")
# can compile-time-embed the SQL files into the binary. Without this COPY the
# macro embeds an empty dir → 0 migrations → the runtime binary creates an empty
# _sqlx_migrations table and no schema ("migrations up to date" but no tables).
COPY cas/migrations ./cas/migrations
# .sqlx offline cache dir — empty is harmless (no sqlx::query! macros in use).
RUN mkdir -p ./cas/.sqlx

# SQLX offline — requires.sqlx/ to be populated by `cargo sqlx prepare`.
# Empty.sqlx/ is harmless for because no `sqlx::query!` macro
# invocation exists yet.
ENV SQLX_OFFLINE=true
# Optional cargo features for the CAS binary. Empty by default (production
# build). The local mock-Sia dev/e2e stack passes CAS_FEATURES=sia-mock to
# compile in the in-memory MockSiaAdapter (gated by OPENWEIGHTS_SIA_MOCK=true
# at runtime). Never set this for a real deployment.
ARG CAS_FEATURES=""
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/cas/target \
    cd cas && cargo build --release --locked --bin openweights-cas \
      ${CAS_FEATURES:+--features "$CAS_FEATURES"} && \
    cp target/release/openweights-cas /openweights-cas

FROM debian:bookworm-slim
RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates wget && \
    rm -rf /var/lib/apt/lists/*
COPY --from=builder /openweights-cas /usr/local/bin/openweights-cas
USER 65532:65532
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/openweights-cas"]
