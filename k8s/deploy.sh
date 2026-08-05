#!/usr/bin/env bash
# Install or upgrade OurDriveway.  Usage: k8s/deploy.sh [local|prod]
#
# Exists for one reason: Helm templates cannot read files outside the chart, and
# the SurrealDB schemas live in schemas/ where the Rust build already uses them.
# `--set-file` carries them across, and four flags is more than fits comfortably
# in a README line. Everything else here is plain helm.
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

# Deliberately no `kubectl rollout restart` here. The projection stores are
# disposable, so restarting a pod throws its database away and replays the whole
# log to rebuild it — too expensive to do on every deploy on the off chance a
# secret moved. Rotating one is an explicit
# `kubectl rollout restart deployment -n ourdriveway`; a schema change rolls
# itself via checksum/schemas in services.yaml.
SET_FILES=()
for s in user booking spot view; do
  SET_FILES+=(--set-file "schemas.$s=schemas/$s-schema.surql")
done

helm upgrade --install ourdriveway k8s/chart \
  --namespace "$NS" \
  --values "k8s/values-$ENV.yaml" \
  "${SET_FILES[@]}" \
  "${@:2}"
