#!/usr/bin/env bash
# Install or upgrade OurDriveway.  Usage: k8s/deploy.sh [local|prod] [extra helm args]
#
# Reads k8s/environments/<env>/:
#   values.yaml        chart overrides
#   config/<svc>.env   non-secret config  → ConfigMap <svc>-service-config
#   secrets.env        secrets (gitignored) → Secret ourdriveway-secrets
#
# Both are created here with kubectl rather than by Helm: the Secret because Helm would
# store it in its release history, the ConfigMaps because a chart cannot read files outside
# itself. A hash of both is passed to the chart so a change rolls the pods.
set -euo pipefail

ENV="${1:-local}"
NS=ourdriveway
SECRET=ourdriveway-secrets   # = secretName in chart/values.yaml
cd "$(dirname "${BASH_SOURCE[0]}")"
DIR="environments/$ENV"

fail() { echo "deploy: $*" >&2; exit 1; }
# The value of key $2 in env file $1.
val() { grep -E "^$2=" "$1" | tail -n 1 | cut -d= -f2-; }
keys() { grep -oE '^[A-Z0-9_]+=' "$1" | tr -d '=' | sort; }

[[ -f "$DIR/values.yaml" ]] || fail "no such environment: $ENV"
[[ -f "$DIR/secrets.env" ]] ||
  fail "$DIR/secrets.env missing — cp secrets.env.example $DIR/secrets.env and fill it in"

# ── secrets ──────────────────────────────────────────────────────────────────
# Exactly the keys the example declares: a missing one would only surface as one pod
# crash-looping, an extra one is almost certainly a typo.
diff <(keys secrets.env.example) <(keys "$DIR/secrets.env" | uniq) >/dev/null ||
  fail "$DIR/secrets.env keys differ from secrets.env.example:
$(diff <(keys secrets.env.example) <(keys "$DIR/secrets.env" | uniq) | grep '^[<>]' | sed 's/^</  missing:/; s/^>/  unknown:/')"
for key in $(keys secrets.env.example); do
  [[ -n "$(val "$DIR/secrets.env" "$key")" ]] || fail "$DIR/secrets.env: $key is empty"
done

# The four this deployment generates itself. Letters and digits only, because the two
# passwords are spliced into DATABASE_URL and NATS_URL unescaped.
invented=(JWT_SECRET EMAIL_TOKEN_SECRET YSQL_PASSWORD NATS_PASSWORD)
for key in "${invented[@]}"; do
  value="$(val "$DIR/secrets.env" "$key")"
  [[ ${#value} -ge 32 && "$value" =~ ^[A-Za-z0-9]+$ ]] ||
    fail "$DIR/secrets.env: $key must be at least 32 letters or digits — openssl rand -hex 32"
done
# All different. JWT_SECRET == EMAIL_TOKEN_SECRET would turn every mailed verification
# link into a bearer token (shared/src/email_token.rs); equal passwords make one leak two.
[[ -z "$(for key in "${invented[@]}"; do val "$DIR/secrets.env" "$key"; echo; done | sort | uniq -d)" ]] ||
  fail "$DIR/secrets.env: ${invented[*]} must all be different values"

# ── config ───────────────────────────────────────────────────────────────────
# Every environment declares the same files and keys, so one cannot lose a variable the
# other still sets.
for other in environments/*/; do
  other="${other%/}"
  [[ "$other" == "$DIR" ]] && continue
  diff <(cd "$DIR/config" && for f in *.env; do keys "$f" | sed "s/^/$f /"; done) \
       <(cd "$other/config" && for f in *.env; do keys "$f" | sed "s/^/$f /"; done) ||
    fail "$DIR/config and $other/config declare different keys (< $ENV, > ${other##*/})"
done

for f in "$DIR"/config/*.env; do
  # kubectl --from-env-file keeps quotes as part of the value.
  ! grep -qE '^[A-Z0-9_]+=["'\'']' "$f" || fail "$f: remove the quotes, kubectl keeps them"
  for key in $(keys "$f"); do
    [[ -n "$(val "$f" "$key")" ]] || fail "$f: $key is empty"
  done
done

# Values that must agree byte for byte across services. MEDIA_BASE: media mints image
# URLs on it, user and spot reject any image not on it. SETTLEMENT_SECS: payment decides
# what a payout hands over, view what the wallet shows as available.
same() {
  local key=$1; shift
  [[ $(for s in "$@"; do val "$DIR/config/$s.env" "$key"; done | sort -u | wc -l) -eq 1 ]] ||
    fail "$DIR/config: $key must be identical in: $*"
}
same MEDIA_BASE user spot media
same SETTLEMENT_SECS payment view

# ── apply ────────────────────────────────────────────────────────────────────
kubectl create namespace "$NS" --dry-run=client -o yaml | kubectl apply -f -

# Apply-from-stdin so each is idempotent.
kubectl create secret generic "$SECRET" --namespace "$NS" \
  --from-env-file="$DIR/secrets.env" --dry-run=client -o yaml | kubectl apply -f -
for f in "$DIR"/config/*.env; do
  kubectl create configmap "$(basename "$f" .env)-service-config" --namespace "$NS" \
    --from-env-file="$f" --dry-run=client -o yaml | kubectl apply -f -
done

# Migrations need no step here: chart/templates/migrator-job.yaml is a Helm hook, and a
# failed migration fails the release before any pod rolls. NATS and YugabyteDB are not
# rolled by the hash — a rotated password there is a manual restart (see README).
helm upgrade --install ourdriveway chart \
  --namespace "$NS" \
  --values "$DIR/values.yaml" \
  --set-string configHash="$(cat "$DIR"/config/*.env "$DIR/secrets.env" | sha256sum | cut -c1-16)" \
  "${@:2}"
