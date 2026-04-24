# syntax=docker/dockerfile:1.7
FROM golang:1.26-bookworm AS builder
WORKDIR /src
COPY hf-proxy/go.mod ./
COPY hf-proxy/*.go ./
RUN --mount=type=cache,target=/root/.cache/go-build \
    --mount=type=cache,target=/go/pkg/mod \
    CGO_ENABLED=0 go build -trimpath -ldflags '-s -w' -o /siahub-hf-proxy .

FROM gcr.io/distroless/static-debian12:nonroot
COPY --from=builder /siahub-hf-proxy /usr/local/bin/siahub-hf-proxy
EXPOSE 28090
ENTRYPOINT ["/usr/local/bin/siahub-hf-proxy"]
