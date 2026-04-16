# Optional packaging as a separate readiness container.
# Default path uses the statically-compiled binary bind-mounted
# into the indexd container (see ops/docker-compose.yml in PLAN 03).
FROM scratch
COPY bin/readiness /readiness
ENTRYPOINT ["/readiness"]
