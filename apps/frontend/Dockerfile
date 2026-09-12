# syntax=docker/dockerfile:1
# The SPA baked into Caddy, so a deployment needs nothing on disk but this image.
#
# Build context is the repo ROOT, not this directory — that is bake's default and
# every other target relies on it. Paths below are therefore repo-relative, and the
# .dockerignore that applies is the one at the repo root (a .dockerignore here
# would be ignored). Everything this image needs now lives under apps/frontend/,
# so narrowing the context is possible — it would just need its own .dockerignore
# to keep node_modules out.
FROM node:alpine AS builder
WORKDIR /app
# Manifests first: this layer only busts when dependencies change, so npm ci is
# skipped on ordinary source edits.
COPY apps/frontend/package.json apps/frontend/package-lock.json ./
RUN npm ci
COPY apps/frontend/ ./
# `npm run build` is `vue-tsc --noEmit && vite build`, so a type error fails the image.
RUN npm run build

FROM caddy:alpine
COPY --from=builder /app/dist /srv/dist
COPY apps/frontend/Caddyfile /etc/caddy/Caddyfile
