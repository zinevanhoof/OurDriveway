# syntax=docker/dockerfile:1
# `builder` is a named build context wired up in docker-bake.hcl (target:builder),
# not an image on a registry. Build via `docker buildx bake`, not `docker build`.
FROM debian:bookworm-slim
# curl is for the compose healthcheck against /readyz — without it the
# `depends_on: condition: service_healthy` clauses can never pass.
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
# Serves ./uploads relative to CWD (/app); mount a volume there to persist it.
COPY --from=builder /out/spot-service /usr/local/bin/service
ENTRYPOINT ["service"]
