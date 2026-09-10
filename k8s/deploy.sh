#!/usr/bin/env bash
# Install or upgrade OurDriveway.  Usage: k8s/deploy.sh [local|prod]
#
# The reason this existed is gone: Helm templates cannot read files outside the chart,
# and the SurrealDB schemas lived in schemas/ where the Rust build already used them, so
# every install carried them across with a `--set-file` per service. Schema is now
# `apps/migrator`'s job — diesel migrations embedded in one binary, applied by the
# Job in migrator-job.yaml — so there is nothing to carry.
#
# What is left is the secret, which is deliberately not templated (see values.yaml), and
# that is still worth a script rather than three README lines.
set -euo pipefail

ENV="${1:-local}"
NS=ourdriveway
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

[[ -f "k8s/values-$ENV.yaml" ]] || { echo "no such environment: $ENV" >&2; exit 1; }
[[ -f k8s/secrets.env ]] || {
  echo "k8s/secrets.env missing — cp k8s/secrets.env.example k8s/secrets.env and fill it in" >&2
  exit 1
}

kubectl create namespace "$NS" --dry-run=client -o yaml | kubectl apply -f -

# Kept out of the chart on purpose: Helm stores every value it renders in the
# release secret and hands them back to anyone who runs `helm get values`, which
# is no place for a JWT signing key. Apply-from-stdin so this is idempotent.
kubectl create secret generic app-secrets \
  --namespace "$NS" \
  --from-env-file=k8s/secrets.env \
  --dry-run=client -o yaml | kubectl apply -f -

# Deliberately no `kubectl rollout restart` here. A pod picks up a rotated secret
# at its next start, and rolling every deployment on the off chance one moved is
# noise. Rotating one is an explicit
# `kubectl rollout restart deployment -n ourdriveway`.
#
# A schema change still needs no separate step here, but it is no longer the services
# doing it: templates/migrator-job.yaml is a `post-install,pre-upgrade` Helm hook, and
# Helm waits for it and fails the release if it does not complete. So this one command
# is still the whole deploy, and a failed migration stops it before any pod rolls.
#
# Centralised on purpose, reversing what this comment used to say. Boot-time migration
# in every replica was safe only because sqlx locked around the run; diesel_migrations
# does not lock, so `replicas: N` would race. See diesel-migration.md.
helm upgrade --install ourdriveway k8s/chart \
  --namespace "$NS" \
  --values "k8s/values-$ENV.yaml" \
  "${@:2}"
