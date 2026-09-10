# syntax=docker/dockerfile:1
# `builder` is a named build context wired up in docker-bake.hcl (target:builder),
# not an image on a registry. Build via `docker buildx bake`, not `docker build`.
FROM debian:bookworm-slim
# No curl, unlike the services: this is a one-shot with no /readyz and nothing that
# healthchecks it. ca-certificates stays for the same reason it does everywhere — the
# connection is plain TCP today, but a cluster that ever turns on sslmode would fail
# here first and most confusingly.
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
# NOT renamed to `service`, unlike every image next to it. This is not a service — it
# runs to completion and exits, and `docker ps` showing `migrator` rather than a seventh
# `service` is the point.
COPY --from=builder /out/migrator /usr/local/bin/migrator
ENTRYPOINT ["migrator"]
