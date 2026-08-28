#!/usr/bin/env bash
# Install or upgrade OurDriveway.  Usage: k8s/deploy.sh [local|prod]
#
# Exists for one reason: Helm templates cannot read files outside the chart, and
# the SurrealDB schemas live in schemas/ where the Rust build already uses them.
# `--set-file` carries them across, and that many flags is more than fits comfortably
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

# Deliberately no `kubectl rollout restart` here. A pod picks up a rotated secret
# at its next start, and rolling every deployment on the off chance one moved is
# noise. Rotating one is an explicit
# `kubectl rollout restart deployment -n ourdriveway`.
#
# A schema change needs no roll at all any more: it used to, because every pod
# carried a SurrealDB sidecar that re-imported at boot, so services.yaml rolled
# them via a checksum annotation. The schemas are applied to the shared datastore
# by the schema-import Job, which re-runs on every upgrade, and no service holds a
# copy to go stale.
SET_FILES=()
for s in user booking spot view payment; do
  SET_FILES+=(--set-file "schemas.$s=schemas/$s-schema.surql")
done

# Untuned, TiKV sizes its block cache to the machine — ~45% of system memory. On a
# k3d cluster that machine is a laptop that is also running a cargo build, which
# CLAUDE.md already pins `jobs = 4` to survive. Same file the compose dev stack
# mounts, rather than a second copy of the same numbers in values-local.yaml.
#
# Deliberately not applied to prod: a real deployment should let TiKV size itself
# to a node it does not share.
if [[ "$ENV" == local ]]; then
  SET_FILES+=(--set-file "tikv.config=docker/tikv/tikv.toml")
fi

helm upgrade --install ourdriveway k8s/chart \
  --namespace "$NS" \
  --values "k8s/values-$ENV.yaml" \
  "${SET_FILES[@]}" \
  "${@:2}"
