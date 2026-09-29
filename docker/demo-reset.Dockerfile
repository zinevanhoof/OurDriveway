# syntax=docker/dockerfile:1
# The public demo's hourly reset: scale the services down, wipe, migrate, seed, scale
# back up. The script is k8s/chart/templates/demo-reset.yaml's; this image only carries
# what it runs: the migrator, the demo seed and kubectl.
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*

# Pinned; any recent one works for `scale` and `get`, which are all the script uses.
ARG KUBECTL_VERSION=v1.36.3
ARG TARGETARCH=amd64
RUN curl -fsSLo /usr/local/bin/kubectl \
        "https://dl.k8s.io/release/${KUBECTL_VERSION}/bin/linux/${TARGETARCH}/kubectl" \
    && chmod +x /usr/local/bin/kubectl

WORKDIR /app
COPY --from=builder /out/migrator /out/demo_seed /usr/local/bin/
