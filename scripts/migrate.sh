#!/usr/bin/env bash
# Create and migrate the five dev databases. The ONLY thing that applies schema in dev.
#
#   docker compose -f docker/docker-compose-dev.yml up -d yugabyte
#   scripts/migrate.sh
#
# Run it after yugabyte reports healthy and BEFORE the "run-all-services" task. A service
# started against an unmigrated database fails fast with 3D000, which is the visible
# failure and is meant to be.
#
# ── Why this is a script and not a compose service ───────────────────────────
#
# It was a compose service twice, and neither shape worked:
#
#   * as a published image (`ghcr.io/…/ourdriveway-migrator:latest`) it only exists after
#     a merge to main, so any other branch got a stale migrator or a registry `denied` —
#     and the schema was whatever the last release said rather than what this tree says;
#   * as a local `build:` it compiled the whole workspace in RELEASE to produce one small
#     binary, minutes of CPU for a one-shot that a debug build runs exactly as correctly.
#
# So: `cargo run` against the same debug target directory `cargo check`/`cargo test`
# already keep warm. Usually seconds, and nothing is rebuilt when nothing changed.
#
# Production is unaffected — there the migrator is the Helm hook Job in
# k8s/chart/templates/migrator-job.yaml, built by docker-bake.hcl like every other image.
# This is a dev-loop decision only.
#
# ── Why one process and not one per service ──────────────────────────────────
#
# `diesel_migrations` takes no lock around a run, unlike `sqlx::migrate`, so services
# migrating at boot would race at `replicas: N`. `migrator::run_all` does all five in
# order, and its exit code is the whole contract: 0 means every database is at its latest
# migration, non-zero means at least one is not.
set -euo pipefail

cd "$(dirname "$0")/.."

HOST="${YSQL_HOST:-127.0.0.1}"
PORT="${YSQL_PORT:-5433}"
USER_NAME="${YSQL_USER:-yugabyte}"

# Named per service because that is what `migrator::run_all` reads — one URL per database,
# and it reports a missing one by name rather than failing anonymously. Exported rather
# than passed, since the binary takes no arguments.
#
# `dotenvy` runs first inside the migrator and a repo-root `.env` would win over these, so
# anything set there stays authoritative; these are the defaults for a plain checkout.
for db in user spot booking payment view; do
    export "$(echo "$db" | tr '[:lower:]' '[:upper:]')_DATABASE_URL=postgres://${USER_NAME}@${HOST}:${PORT}/${db}"
done

echo "migrating five databases on ${HOST}:${PORT}"

# Debug, deliberately: see the header. `-p migrator` rather than `--workspace` because
# this builds one binary and the two artifact sets coexist — see CLAUDE.md.
exec cargo run -p migrator
