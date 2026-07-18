# syntax=docker/dockerfile:1
# One image for every backend service; pick which with `--build-arg SERVICE=<crate>`.
# BuildKit cache mounts give ALL services one shared target/ + cargo registry, so
# common deps compile once and rebuilds only recompile changed crates.
FROM rust:bookworm AS builder
RUN apt-get update && apt-get install -y --no-install-recommends cmake \
    && rm -rf /var/lib/apt/lists/*
# ^ cmake for ring/aws-lc-sys; gcc + perl already ship in the rust image.
WORKDIR /app
COPY . .
ARG SERVICE
# --bin keeps the Tauri workspace member (needs webkit2gtk, absent here) from building.
# sharing=locked: compose builds services in parallel, but cargo can't share one
# target/ concurrently — serialize the compile step while still sharing the cache.
RUN --mount=type=cache,target=/app/target,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    cargo build --release --bin ${SERVICE} && cp target/release/${SERVICE} /out

FROM debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
# spot-service serves ./uploads relative to CWD; mount a volume here to persist it.
COPY --from=builder /out /usr/local/bin/service
ENTRYPOINT ["service"]
