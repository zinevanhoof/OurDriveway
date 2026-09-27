# Deploying OurDriveway on Kubernetes

Plain Helm, one chart, one directory per environment.

```
k8s/
  deploy.sh                the whole deploy: validates, creates Secret + ConfigMaps, runs Helm
  secrets.env.example      the template for every environment's secrets.env
  chart/
    values.yaml            chart defaults — no env values, only shape
    templates/
      backends.yaml        every service: Deployment + Service, from `services` in values
      frontend.yaml        Caddy serving the SPA
      ingress.yaml         the one host: / → frontend, /api/<name> → <name>-service
      rate-limit.yaml      Traefik middlewares the ingress attaches
      migrator-job.yaml    Helm hook: creates and migrates every database
      yugabyte.yaml        the database, single node
      network-policy.yaml  who may reach YSQL and NATS
  environments/
    local/  prod/
      values.yaml          chart overrides for this environment
      config/<svc>.env     non-secret config, one file per service (committed)
      secrets.env          secrets (gitignored)
```

## Configuration

**Every environment variable a service reads lives in a file, never in the chart** —
the same split as `apps/services/<svc>/.env`, one file per service:

| Where | What | Becomes |
|---|---|---|
| `environments/<env>/config/<svc>.env` | non-secret config (URLs, TTLs, bucket, limits) | ConfigMap `<svc>-service-config` |
| `environments/<env>/secrets.env` | credentials | Secret `ourdriveway-secrets`, one key at a time per the `secrets` lists in `chart/values.yaml` |
| `chart/templates/backends.yaml` | `PORT`, `DATABASE_URL`, `NATS_URL` | wiring the chart owns, built from the Secret's passwords |

Together those are exactly each service's `Config` struct. Config has no defaults, so a
missing variable crashes the pod at startup (`kubectl logs` names it).

`deploy.sh` refuses to touch the cluster when:

- `secrets.env` has a key missing, empty or not in `secrets.env.example`;
- a generated secret is short, not alphanumeric, or equal to another;
- two environments' config files declare different keys (so one cannot quietly miss a
  variable the other sets);
- a config value is quoted (`kubectl --from-env-file` keeps quotes as part of the value) or empty;
- `MEDIA_BASE` differs between user, spot and media, or `SETTLEMENT_SECS` between payment
  and view — those must agree byte for byte.

Adding a variable: add it to the service's `Config`, its dev `.env`, and
`environments/*/config/<svc>.env`. A new secret additionally goes in
`secrets.env.example`, every `secrets.env`, and the service's `secrets` list in
`chart/values.yaml`.

Changing either file and re-running `deploy.sh` rolls the backends: it passes a hash of
the environment's config and secrets to the chart as a pod annotation. YugabyteDB and NATS
are not rolled by it — and `YSQL_PASSWORD` only takes effect at the database's first boot,
so rotating it means changing the role's password in YSQL too.

The Secret is created by kubectl rather than templated because Helm stores every rendered
value in its release secret and prints it back with `helm get values`.

## Once

```sh
helm repo add nats https://nats-io.github.io/k8s/helm/charts/
helm dependency update k8s/chart          # vendors the NATS subchart
cp k8s/secrets.env.example k8s/environments/local/secrets.env
$EDITOR k8s/environments/local/secrets.env
```

Upgrading a cluster deployed before the rename: `kubectl delete secret app-secrets -n
ourdriveway` once the new release is up.

## TLS

The chart references a `ourdriveway-tls` Secret when `ingress.tls.enabled` is on, but
does not create it — a certificate is not something a chart should mint.

Behind Cloudflare, use a **Cloudflare Origin CA** certificate: free, valid 15 years,
trusted only by Cloudflare, issued under SSL/TLS → Origin Server.

```sh
kubectl create secret tls ourdriveway-tls \
  --cert=origin.pem --key=origin.key -n ourdriveway
```

Set Cloudflare's SSL/TLS mode to **Full (strict)**. Then:

- **Lock the origin down.** Restrict the VPS firewall to Cloudflare's IP ranges on
  80/443. Otherwise anyone who learns the address bypasses the proxy, sees a cert no
  browser trusts, and can forge `CF-Connecting-IP` to dodge the rate limits.
- **Client IPs arrive in headers.** Rate limiting reads `CF-Connecting-IP`
  (`rateLimit.sourceHeader` in prod); logs show Cloudflare's addresses unless Traefik's
  `forwardedHeaders.trustedIPs` is set to those ranges.

## A local cluster

k3d is k3s in Docker: same distribution and same bundled Traefik as the VPS.

```sh
k3d cluster create ourdriveway -p "80:80@loadbalancer" -p "443:443@loadbalancer"

# Your own images. Skip this to run what CI last published from main.
docker buildx bake --load
for i in user-service booking-service spot-service view-service media-service \
         notification-service payment-service migrator frontend; do
  k3d image import ghcr.io/zinevanhoof/ourdriveway-$i:latest -c ourdriveway
done

k8s/deploy.sh local
```

Then http://localhost here, or http://192.168.50.29 (this machine's LAN IP) from a phone.
The local Ingress has no host, so it answers on any address. Local config differs from
prod in what plain HTTP on a LAN needs: `COOKIE_SECURE=false`, the `ourdriveway-dev`
bucket and its public URL, and `APP_BASE_URL` on the LAN IP so emailed links open on a
phone — change it in `environments/local/config/notification.env` if the IP moves.
`environments/local/values.yaml` also caps YugabyteDB's memory, which otherwise sizes
itself to the whole machine.

`kubectl get pods -n ourdriveway -w` while it comes up: `yugabyte`, then the services. A
pod in `Init` is waiting (`kubectl logs <pod> -c <init-container>` says for what). On a
**first** install a service may restart once: its init container proves YugabyteDB is
listening, not that the migrator has created its database yet, and the retry succeeds.

Teardown: `k3d cluster delete ourdriveway`.

## Deploy

```sh
k8s/deploy.sh local      # or: k8s/deploy.sh prod
```

Extra arguments go to Helm, so `k8s/deploy.sh prod --dry-run` works.

**Migrations** are `chart/templates/migrator-job.yaml`, a `post-install,pre-upgrade` Helm
hook: Helm waits for it and fails the release before any pod rolls if it fails. One Job,
not every replica at boot, because `diesel_migrations` takes no lock. It is not
`pre-install` because on a cold install the yugabyte StatefulSet would not exist yet —
which is also why a first install can see the one restart above.

## Routing and load balancing

A ClusterIP Service fronts each backend and kube-proxy spreads connections across ready
pods, so scaling is `replicas: N`. `readinessProbe: /readyz` keeps a pod that has not
caught up on its projections out of rotation; for user, spot and payment, which project
nothing, it only means NATS is reachable.

The Ingress owns all routing on **one host**, so the `SameSite=Strict` refresh cookie
works: `/api/<name>` to each service with an API (all but notification), everything else
to the frontend. Stripe's webhook arrives at `/api/payment/webhook` through the same
Ingress — the one route with no JWT; it authenticates by signature (`route/webhook.rs`).

## Where the state lives

```
services  ──►  yugabyte (StatefulSet, one database per service)
   │                    ▲
   └──►  NATS JetStream ─┘ integration events, kept forever (rebuild path)
```

| Volume | Holds | Losing it |
|---|---|---|
| `yugabyte` | every service database and the read model | **data loss** |
| NATS | every event on USERS, SPOTS, BOOKINGS, PAYMENTS (SESSIONS: 31 days) | no data loss, but projections can no longer be rebuilt |

**YugabyteDB is authoritative.** The log rebuilds projections, not the owning services'
rows — they hold things no event carries (password hashes) — so deleting the `yugabyte`
PVC is data loss, not a replay.

The NATS PVC grows forever: the four event streams have no `max_age` or `max_bytes`
(`shared::events::STREAMS`). When it fills, publishes are refused and every outbox holds
its rows and retries — writes still commit, projections stop. Watch it and resize first.

### Projectors

Each projector holds **one durable consumer per stream**, shared by every replica, so
throughput scales with `replicas`. Each replica applies up to
`bus::projector::APPLY_CONCURRENCY` events at once. Ordering comes from the data, not the
transport: `bus::projector::decide` reads the aggregate's stored `version` under
`FOR UPDATE`, applies only the next one, acks what is already stored, and `Nak`s events
from the future so they are redelivered once the gap closes.

The database connection bill is `APPLY_CONCURRENCY` × projectors. `max_size` in
`shared/src/db.rs` is the ceiling; a pool timeout stops the projector and 503s the pod.

### Rebuilding a projection

view-service's database is derived from user, spot, booking and payment; if it disagrees
with them, they are right. Rebuild it by replaying:

1. `kubectl scale -n ourdriveway deploy/view-service --replicas=0`
2. Empty every table in the `view` database except diesel's migration bookkeeping, or drop
   the database and let the migrator job recreate it.
3. Delete its four durable consumers:
   ```sh
   PASS=$(kubectl get secret -n ourdriveway ourdriveway-secrets -o jsonpath='{.data.NATS_PASSWORD}' | base64 -d)
   kubectl run -n ourdriveway --rm -it nats-box --image=natsio/nats-box \
     --labels=ourdriveway.com/nats-client=true \
     --env="NATS_URL=nats://ourdriveway:$PASS@ourdriveway-nats:4222" -- sh -c '
       sleep 5
       for pair in USERS:view-users SPOTS:view-spots BOOKINGS:view-bookings PAYMENTS:view-payments; do
         nats consumer rm "${pair%%:*}" "${pair##*:}" -f
       done'
   ```
   The label gets the pod past `network-policy.yaml`; the `sleep` gives k3s time to admit
   its IP.
4. Scale back up. Each projector recreates its consumer from sequence 1, and `/readyz`
   stays 503 until the replay has caught up.

The same works for `payment-booking-mirror`, `payment-host-mirror` and `booking-spots`.
**Never delete a worker's durable** (`notification-users`, `booking-payments`,
`payment-bookings`, `payment-payments`): it is recreated at `DeliverPolicy::New` and
silently skips whatever was pending.

## Known ceilings

- **Single YugabyteDB node**, one copy of the authoritative data, and no backup story.
  Multi-node needs `--join` and a replication factor — a different template, not
  `replicas: 3`. Add it and a backup (`ysql_dump` or YugabyteDB snapshots) before
  anything real lives here.
- **Single NATS node** holding the only rebuild source. Set `nats.config.cluster.enabled`
  with three replicas, or back up the PVC, before that matters.
- **Every event ever published must keep decoding.** A replay reads the whole history, so
  an incompatible event change breaks the next rebuild, not the next deploy. Add fields
  with `#[serde(default)]`; never rename or remove one.
- **The log is not a ledger.** The record of charges and refunds is the `payment` and
  `payout` tables; if they disagree with `STREAM_PAYMENTS`, the tables are right.
- **NetworkPolicy needs a CNI that enforces it.** k3s does; on any other cluster check
  before counting on `network-policy.yaml`.
