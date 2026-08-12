# syntax=docker/dockerfile:1
# Compiles the whole workspace once and exposes the five binaries at /out.
# Not a deployable image — the per-service Dockerfiles consume it as a named
# build context (see docker-bake.hcl), so bake resolves it a single time for
# all five and the Rust workspace compiles once.
FROM rust:bookworm AS build
RUN apt-get update && apt-get install -y --no-install-recommends cmake \
    && rm -rf /var/lib/apt/lists/*
# ^ cmake for ring/aws-lc-sys; gcc + perl already ship in the rust image.
WORKDIR /app
COPY . .
# --workspace, never per-service --bin/-p: resolver v2 unifies features over the
# selected packages, so a single-package selection is a different feature set and
# recompiles the shared dep tree. This matches `cargo test --workspace` and the
# VS Code build task. Safe now that src-tauri is its own workspace.
RUN --mount=type=cache,target=/app/target,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    cargo build --release --workspace \
    && mkdir /out \
    && cp target/release/user-service target/release/booking-service \
          target/release/spot-service target/release/view-service \
          target/release/media-service target/release/notification-service /out/
# ^ the cp is required: /app/target is a cache mount, scratch space that never
# lands in a layer. Only what reaches /out is visible to COPY --from=builder.

# scratch, so consuming this context costs nothing but the binaries themselves.
FROM scratch
COPY --from=build /out/ /out/
