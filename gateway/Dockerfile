# siahub-gateway — multi-stage build.
# Wave 1 scaffold. Image produces a static binary + distroless runtime.
# Build: docker build -t siahub-gateway:dev gateway/
# Run (dev): docker run --rm -p 8081:8081 \
# -e GATEWAY_URL_SIGNING_KEY="$(head -c32 /dev/urandom | base64)" \
# siahub-gateway:dev
# The /metrics listener (default 127.0.0.1:9100) is DELIBERATELY not exposed
# Caddy MUST NOT proxy it. Prometheus scrapes it via docker-compose exec
# or a sidecar on the same docker network.
# syntax=docker/dockerfile:1.7

FROM golang:1.26-alpine AS build
WORKDIR /src

# Dependency layer — cacheable.
COPY go.mod go.sum* ./
RUN go mod download

# Source layer. `*_test.go` + any markdown excluded via.dockerignore.
COPY . .

# Static build. `CGO_ENABLED=0` + `-tags netgo` keeps the binary self-contained
# for distroless. `-s -w` strips debug info.
RUN CGO_ENABLED=0 GOOS=linux GOARCH=amd64 \
    go build -tags netgo -ldflags='-s -w' -o /out/gateway .

# Distroless static — no shell, no libc, UID 65532 (nonroot).
FROM gcr.io/distroless/static-debian12:nonroot
USER 65532:65532
COPY --from=build /out/gateway /gateway
# Cache dir consumed by ; declared here so docker-compose can bind-mount.
VOLUME ["/var/cache/siahub"]
EXPOSE 8081
ENTRYPOINT ["/gateway"]
