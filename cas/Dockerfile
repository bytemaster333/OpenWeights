# syntax=docker/dockerfile:1.7
FROM rust:1.85-bookworm AS builder
WORKDIR /src

# Copy workspace manifests first for cached dep layer.
COPY cas/Cargo.toml cas/rust-toolchain.toml ./cas/
COPY cas/crates ./cas/crates

# migrations/ must be present at build time so sqlx::migrate!("./migrations") can
# compile-time-embed the SQL files into the binary. Plan 02-02 populates these.
# For Plan 02-01 the directory may not yet exist; create an empty one so the
# COPY succeeds.
RUN mkdir -p ./cas/migrations ./cas/.sqlx

# Attempt to copy real migrations + sqlx offline cache if they exist; the
# wildcard trick keeps the COPY from failing when Plan 02-02 hasn't landed.
# At runtime `sqlx::migrate!` resolves the path at compile time.
# NOTE: because Dockerfile COPY does not support optional sources, Plan 02-02's
# migrations/ and .sqlx/ are written unconditionally and this stanza becomes
# purely load-bearing once Plan 02-02 runs.

# SQLX offline — requires .sqlx/ to be populated by `cargo sqlx prepare`.
# Empty .sqlx/ is harmless for Plan 02-01 because no `sqlx::query!` macro
# invocation exists yet.
ENV SQLX_OFFLINE=true
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/cas/target \
    cd cas && cargo build --release --bin siahub-cas && \
    cp target/release/siahub-cas /siahub-cas

FROM debian:bookworm-slim
RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates wget && \
    rm -rf /var/lib/apt/lists/*
COPY --from=builder /siahub-cas /usr/local/bin/siahub-cas
USER 65532:65532
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/siahub-cas"]
