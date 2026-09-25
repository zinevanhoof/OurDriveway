# syntax=docker/dockerfile:1
# `builder` is a named build context wired up in docker-bake.hcl (target:builder),
# not an image on a registry. Build via `docker buildx bake`, not `docker build`.
FROM debian:bookworm-slim
# curl is for the compose healthcheck against /readyz — without it the
# `depends_on: condition: service_healthy` clauses can never pass.
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
# Every Argon2 hash allocates one 19 MiB buffer (see src/auth/password.rs). glibc's
# mmap threshold is *dynamic*: after the first such buffer is freed it rises to 19 MiB,
# so later ones come from per-thread arenas that keep the memory after free. Measured
# under 50 concurrent logins on 4 cores: 766–785 MiB peak per pod, ~410 MiB still held
# after the burst. Setting the threshold pins it, so every buffer is mmapped and
# returned on free: 82 MiB peak, back to 9 MiB idle, same throughput.
ENV MALLOC_MMAP_THRESHOLD_=131072
COPY --from=builder /out/user-service /usr/local/bin/service
ENTRYPOINT ["service"]
