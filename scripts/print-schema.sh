#!/usr/bin/env bash
# Regenerate every service's diesel schema module from the LIVE dev cluster.
#
#   docker compose -f docker/docker-compose-dev.yml up -d yugabyte
#   scripts/migrate.sh
#   scripts/print-schema.sh
#
# The schema is read back out of the database, so it needs a running cluster with the
# migrations already applied — the .sql files are not parsed. Run this after adding a
# migration, or the generated module and the database drift apart. Nothing in CI checks
# that (there is no database there); a stale module surfaces as a compile error at the
# query that uses the missing column.
#
# There is no `--config-name` (the flag is `--schema-key`), and named per-database
# sections are not used anyway — see the note in diesel.toml. One shared `[print_schema]`
# section; five invocations for the service modules, then five more for bus's two tables
# (which are cross-checked against each other and written once).
#
# ── schema-patches/ ──────────────────────────────────────────────────────────
#
# `diesel print-schema` emits `Array<Nullable<Text>>` for every Postgres array, because
# `NOT NULL` constrains the column and not its elements. `images` and `license_plates`
# never hold a NULL element, so the three modules that have them are patched to
# `Array<Text>` and the fields decode as a plain `Vec<String>`.
#
# `patch_file` cannot live in diesel.toml here: that key is per-section, and print-schema
# emits EVERY configured section on every invocation (diesel_cli `main.rs`, and
# `--schema-key` only overrides settings on a section rather than selecting one), so
# per-database sections would give five concatenated copies. The identical `--patch-file`
# FLAG applies to the single section, which lets the patch vary per database the same way
# `--database-url` already does.
#
# Only three databases have arrays; the other two have no patch and the flag is omitted.
#
# The patches are applied with `diffy`, which has NO fuzz — the three context lines either
# side must match exactly, so adding a column next to a patched one breaks the run with
# "error applying patch" — loudly, and with the committed module left untouched. To
# regenerate that db's patch, take an UNpatched dump and diff it against a sed of itself:
#
#   diesel print-schema --database-url postgres://yugabyte@localhost:5433/<db> > /tmp/a
#   sed 's/Array<Nullable<Text>>/Array<Text>/g' /tmp/a > /tmp/b
#   diff -u /tmp/a /tmp/b > scripts/schema-patches/<db>.patch
#
# The CLI is a dev tool only: production embeds its migrations in the migrator image and
# never installs it. Get it with:
#
#   cargo install diesel_cli --no-default-features --features postgres-bundled
#
# `postgres-bundled` rather than `postgres` builds libpq from source, so this needs no
# `libpq-dev` on the machine.
set -euo pipefail

cd "$(dirname "$0")/.."

HOST="${YSQL_HOST:-localhost}"
PORT="${YSQL_PORT:-5433}"
USER_NAME="${YSQL_USER:-yugabyte}"

mkdir -p shared/src/schema

for db in user spot booking payment view; do
  echo "print-schema: $db"
  patch=()
  if [[ -f "scripts/schema-patches/${db}.patch" ]]; then
    patch=(--patch-file "scripts/schema-patches/${db}.patch")
  fi
  # Through a temp file, not a redirect onto the module: `>` truncates before diesel
  # runs, so a patch that no longer applies would leave an EMPTY schema module behind
  # and `set -e` would abort before anything could rewrite it.
  tmp="$(mktemp)"
  diesel print-schema \
    --database-url "postgres://${USER_NAME}@${HOST}:${PORT}/${db}" \
    "${patch[@]}" \
    > "$tmp"
  mv "$tmp" "shared/src/schema/${db}.rs"
done

# ── bus's two tables, generated once ─────────────────────────────────────────
#
# `_lease` and `_outbox` are filtered out of the five service modules by diesel.toml and
# generated here instead, so each table has exactly one declaration and it lives in the
# crate that owns it. `--only-tables` REPLACES the config's `except_tables` rather than
# combining with it, which is what lets one section serve both passes.
#
# Every database has an identical copy of both, because every service runs its own outbox
# relay and leader election — the tables are created five times over by
# `migrations/<svc>/0001_init/up.sql`. So all five are read and compared: they must agree,
# and if a migration ever drifts from its siblings this is the one place that notices.
# That check is why this reads five databases to write one file.
echo "print-schema: bus (_lease, _outbox)"
prev=""
for db in user spot booking payment view; do
  tmp="$(mktemp)"
  diesel print-schema \
    --database-url "postgres://${USER_NAME}@${HOST}:${PORT}/${db}" \
    --only-tables _lease _outbox \
    > "$tmp"
  if [[ -n "$prev" ]] && ! diff -q "$prev" "$tmp" >/dev/null; then
    echo "bus tables differ between databases — a migration has drifted:" >&2
    diff -u "$prev" "$tmp" >&2
    exit 1
  fi
  if [[ -n "$prev" ]]; then
    rm -f "$prev"
  fi
  prev="$tmp"
done
mv "$prev" bus/src/schema.rs

echo "schema modules regenerated under shared/src/schema/ and bus/src/schema.rs"
