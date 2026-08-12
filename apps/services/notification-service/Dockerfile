# syntax=docker/dockerfile:1
# `builder` is a named build context wired up in docker-bake.hcl (target:builder),
# not an image on a registry. Build via `docker buildx bake`, not `docker build`.
FROM debian:bookworm-slim
# ca-certificates is load-bearing here, not boilerplate: this service's only
# outbound call is HTTPS to the Resend API, and without a trust store every send
# fails certificate verification. curl is for the compose healthcheck.
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=builder /out/notification-service /usr/local/bin/service
ENTRYPOINT ["service"]
