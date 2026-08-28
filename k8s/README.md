# Deploying OurDriveway on Kubernetes

Plain Helm. No Kustomize — `kubectl kustomize` / `kubectl apply -k` uses kubectl's
embedded copy, which omits `--enable-helm` and `--load-restrictor`, so it can
neither inflate a chart nor read `schemas/` from outside `k8s/`. Helm covers
everything we wanted Kustomize for anyway: `--set-file` crosses the chart
boundary and values files replace overlays.

```
k8s/
  chart/              the chart — every backend is data in values.yaml,
                      one template in templates/services.yaml;
                      storage is tikv.yaml + surrealdb.yaml + schema-import.yaml
  values-local.yaml   dev cluster: no TLS, 1 replica each
  values-prod.yaml    TLS, 2 replicas
  deploy.sh
  secrets.env.example
```

## Once

```sh
helm repo add nats https://nats-io.github.io/k8s/helm/charts/
helm dependency update k8s/chart          # vendors the NATS subchart
cp k8s/secrets.env.example k8s/secrets.env && $EDITOR k8s/secrets.env
```

No minimum Kubernetes version beyond what the API objects here need. The old
**≥ 1.29** floor was for the native sidecar that ordered a per-pod SurrealDB ahead
of the app; there are no sidecars left, only plain init containers.

## TLS

The chart references a `ourdriveway-tls` Secret when `ingress.tls.enabled` is on,
but deliberately does not create it — a certificate is not something a chart
should mint.

Behind Cloudflare, the cheapest option by a wide margin is a **Cloudflare Origin
CA** certificate: free, valid 15 years, trusted only by Cloudflare, issued from
the dashboard under SSL/TLS → Origin Server.

```sh
kubectl create secret tls ourdriveway-tls \
  --cert=origin.pem --key=origin.key -n ourdriveway
```

Set the Cloudflare SSL/TLS mode to **Full (strict)**. No cert-manager, no ACME
resolver, no HTTP-01 challenge, no renewal job. Without Cloudflare, Traefik's own
ACME resolver or cert-manager are the alternatives, and both cost more moving
parts than this.

Two things that come with putting Cloudflare in front:

- **Lock the origin down.** Cloudflare only proxies HTTP(S) on standard ports, so
  restrict the VPS firewall to Cloudflare's published IP ranges on 80/443.
  Otherwise anyone who learns the server's address bypasses the proxy entirely
  and the Origin CA cert — which no browser trusts — starts throwing warnings.
- **Client IPs arrive in headers, not the connection.** Every request appears to
  come from Cloudflare unless Traefik is told to trust `X-Forwarded-For` from
  those ranges (`forwardedHeaders.trustedIPs`). Nothing here rate-limits by IP
  today, so this only matters once something does — but logs are wrong until
  it's set.

## A local cluster

k3d, because it *is* k3s in Docker: same distribution and same bundled Traefik as
a VPS, so local and production run the same ingress controller rather than two
that differ in exactly the details that bite you.

```sh
# 1. k3d (see https://k3d.io). Traefik comes with it — nothing else to install.
k3d cluster create ourdriveway -p "80:80@loadbalancer" -p "443:443@loadbalancer"

# 2. Your own images. ghcr only has what CI published from main, so build the
#    working tree and hand the results straight to the cluster.
docker buildx bake --load
for i in user-service booking-service spot-service view-service media-service \
         notification-service payment-service frontend; do
  k3d image import ghcr.io/zinevanhoof/ourdriveway-$i:latest -c ourdriveway
done

# 3. Deploy, and resolve the host.
k8s/deploy.sh local
echo "127.0.0.1 ourdriveway.local" | sudo tee -a /etc/hosts
```

Then http://ourdriveway.local. `kubectl get pods -n ourdriveway -w` while it comes
up, and expect it in this order: `pd` → `tikv` → `surrealdb` → the `schema-import`
Job → the services. Each stage waits on the one before it in an init container —
this chart's `depends_on` — so a cold cluster should reach Running with **zero
restarts**. A pod stuck in `Init` is waiting, not broken; `kubectl logs <pod> -c
<init-container>` says what for.

`k8s/deploy.sh local` also passes `docker/tikv/tikv.toml` to TiKV, which caps the
block cache at 128 MB. Untuned, TiKV sizes itself to the machine — about 45% of
system memory — and a k3d cluster shares that machine with your build.

Skip step 2 to run the last images CI published instead — they pull fine, they
are just whatever was last merged to `main`.

Teardown: `k3d cluster delete ourdriveway`.

### Why not ingress-nginx

It was retired; `github.com/kubernetes-sigs/ingress-nginx` now 404s, though its
Helm index is still served. Nothing in this chart depends on a specific
controller any more — `ingress.annotations` is empty, and the upload ceiling that
used to be an nginx annotation now lives in spot-service as `MAX_UPLOAD_BYTES`.

## Deploy

```sh
k8s/deploy.sh local      # or: k8s/deploy.sh prod
```

The script creates the namespace, applies the Secret from `k8s/secrets.env`, and
runs `helm upgrade --install` with the five `--set-file` flags that carry the
schemas in — plus, for `local` only, the TiKV memory tuning. Extra arguments are
passed through, so `k8s/deploy.sh prod --dry-run` works. Point `ourdriveway.local`
at your ingress controller's IP in `/etc/hosts`.

The schemas are applied by a `schema-import` Job — a Helm hook, so it re-runs on
every upgrade — rather than by each pod at boot. Against one shared TiKV keyspace,
N instances issuing the same `DEFINE … OVERWRITE` concurrently is a write conflict:
measured, six concurrent imports of one schema produced 90 errors, one sequential
import produces zero. A schema change therefore needs no `rollout restart`; nothing
holds a copy to go stale.

The Secret is created by kubectl rather than templated, because Helm stores every
value it renders in the release secret and hands them back to anyone who runs
`helm get values` — no place for a signing key. The cost is that rotating it
needs an explicit `kubectl rollout restart deployment -n ourdriveway`.

## What load-balances what

Nothing in Caddy any more. A ClusterIP Service sits in front of each service's
pods and kube-proxy spreads connections across the ready ones, so scaling is
`replicas: N` in a values file and nothing else. `readinessProbe: /readyz` keeps a
pod that has not caught up out of that endpoint list — the job the old
`health_uri /readyz` checks in the Caddyfile were doing, done a layer lower where
it actually works.

That probe answers much sooner than it used to. A pod no longer rebuilds a
database before it can serve — its rows are already in TiKV — so what is left to
wait for is a foreign projection, and user-, spot- and payment-service register no
streams at all: `/readyz` there reduces to "is NATS reachable", which the outbox
relay still needs.

The Ingress owns all routing: one `/api/<name>` prefix per service that declares
`api` (every one but notification), everything else to Caddy. One host, so the
browser stays on one origin and the `SameSite=Strict` refresh cookie keeps working.

One exception worth knowing: Stripe's webhook arrives at `/api/payment/webhook`
through this same Ingress, and it is the only route in the system that carries no
JWT. It authenticates by signature instead — see `route/webhook.rs`.

## Where the state lives

```
services  ──►  surrealdb (Deployment, stores nothing)  ──►  TiKV + PD (StatefulSets)
   │                                                            ▲
   └──►  NATS JetStream ── integration events, 7 days ──────────┘ (rebuild path)
```

**TiKV is the authoritative store, and the only thing here whose loss is data
loss.** Everything else is derived from it or replaceable.

This used to be the other way round. Every service pod carried its own SurrealDB
sidecar on an `emptyDir`, because `bus/src/projector.rs` gave each instance an
*ephemeral* consumer that replayed the whole log into a private database and kept
its cursor there — so `replicas: N` had to mean N databases, and the log was the
source of truth. None of that holds now: services write their own rows inside the
request's transaction, projectors share durable consumers, and the log is a
seven-day bus. So the sidecars, the `emptyDir`s and the schema mounts are gone and
every service pod is an app container and nothing else.

### Projector throughput is `PARTITIONS`, not `replicas`

Each stream is cut into `shared::events::PARTITIONS` (16) lanes. NATS assigns a
lane from the subject as it stores each message, and a projector runs one durable
consumer per lane with `max_ack_pending: 1` — so one aggregate's events stay
strictly ordered while unrelated ones apply concurrently.

Every replica opens every lane, and a lane's durable hands its one in-flight
message to whichever replica pulled first. Nothing assigns partitions, nothing
rebalances, and a dead replica's lane is picked up by another after `ack_wait`.

The consequence for scaling: **16 partitions means at most 16 concurrent applies
per stream across the whole deployment, however many pods run.** `replicas: N` buys
availability and request throughput; raising `PARTITIONS` is what buys projection
throughput. It is a `const` in `shared`, not a value, because every service
declares the streams at boot and they must agree.

It is also what a projector costs in SurrealDB connections: one per lane, so 16 per
projector and 64 from view-service, which runs four. They cannot be sessions on a
shared socket — `Surreal::clone` races its own sign-in, and a warmed clone deadlocks
behind another lane's open transaction. Both were observed; see the note in
`shared/src/db.rs`. Raising `PARTITIONS` raises the connection count with it.

Changing it needs the streams **drained** first — every `-pNN` durable at 0 pending
in `nats consumer report <STREAM>` — and then a deploy. The stream config is
reconciled on boot, but messages already stored keep the subject they were written
with, so a key that moves lanes would have its older events in one lane and its newer
ones in another, with no order between them. Drained, there is nothing left to race.
Projectors normally sit at the head, so this is a check rather than a wait.

Two consequences worth knowing:

- **`docker compose down -v` / deleting the TiKV PVCs is data loss**, not a
  replay. There is no log to rebuild from any more.
- **Kubernetes ≥ 1.29 is no longer required.** That floor existed for the native
  sidecar (`initContainer` + `restartPolicy: Always`) that ordered SurrealDB ahead
  of the app inside one pod. Every init container in this chart is now a plain one.

### The PVCs

| Volume | Holds | Losing it |
|---|---|---|
| `tikv` | every service database and the read model | **data loss** |
| `pd` | cluster metadata and region placement | data loss (TiKV cannot be read without it) |
| NATS | up to 7 days of undelivered events | consumers miss what they had not read; re-derive with the backfill below |

### Rebuilding a projection

view-service's database is durable but **not authoritative** — it is derived from
user, spot, booking and payment, and if it ever disagrees with them they are right.
Each of those exposes an internal re-emit of its current state:

```sh
kubectl exec -n ourdriveway deploy/user-service -- \
  curl -sf -XPOST localhost/internal/backfill
```

`/internal/backfill` is outside `/api`, which is what keeps it off the ingress —
the Ingress routes `/api/<service>` and sends everything else to the SPA, so
nothing outside the cluster can reach it. Note it is *unauthenticated inside* the
cluster; see the handler's doc comment.

Re-running one inside two minutes is a no-op by design: backfill event ids are
deterministic and land inside the stream's `duplicate_window`.

## Known ceilings

- **Single TiKV store, single PD.** `tikv.replicas: 1` means one copy of the
  authoritative data — fine for a demo, a data-loss risk in production. PD's
  `max-replicas` is derived from that count and capped at 3, so raising
  `tikv.replicas` to 3 is genuinely all it takes. Do that before anything real
  lives here, and add a backup story (TiKV's own BR); nothing in this repository
  has ever needed one before.
- **SurrealDB is reachable from every pod in the namespace.** It was loopback-only
  when it was a sidecar. The root credentials in `app-secrets` are now the only
  control; a NetworkPolicy admitting just the service pods is the next rung.
- **Single NATS node.** Set `nats.config.cluster.enabled` and three replicas the
  day an outage would mean losing undelivered events rather than downtime.
- **No resource limits, only requests.** Enough to schedule sensibly, not enough
  to stop a runaway pod from starving a node.
- **The log is no longer a ledger.** `STREAM_PAYMENTS` used to keep every money
  event forever; it expires after seven days now and that record lives in the
  `payment` and `payout` tables. An *independent* audit trail, if wanted, has to be
  built as one rather than recovered by turning retention off.
