# syntax=docker/dockerfile:1
# The SPA baked into Caddy, so the VPS needs nothing on disk but the compose file.
#
# Build context is the repo ROOT, not this directory — the Caddyfile it ships with
# lives in docker/. Paths below are therefore repo-relative, and the .dockerignore
# that applies is the one at the repo root (a .dockerignore here would be ignored).
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
COPY docker/Caddyfile /etc/caddy/Caddyfile
