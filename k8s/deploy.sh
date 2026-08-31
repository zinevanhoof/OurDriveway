#!/usr/bin/env bash
# Install or upgrade OurDriveway.  Usage: k8s/deploy.sh [local|prod]
#
# The reason this existed is gone: Helm templates cannot read files outside the chart,
# and the SurrealDB schemas lived in schemas/ where the Rust build already used them, so
# every install carried them across with a `--set-file` per service. Schemas are sqlx
# migrations embedded in each service binary now — there is nothing to carry.
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
# A schema change needs no roll and no separate step: each service applies its own
# migrations at boot from `sqlx::migrate!`, so a new image carries its schema with it.
# Nothing about the database is done centrally any more — a service whose database does
# not exist creates it and retries, in `shared::db::connect`. That deleted the
# create-databases post-install hook and its second copy of the service list.
helm upgrade --install ourdriveway k8s/chart \
  --namespace "$NS" \
  --values "k8s/values-$ENV.yaml" \
  "${@:2}"
