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
                      storage is yugabyte.yaml, and that is all of it
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
up, and expect it in this order: `yugabyte` → the services, each creating and
migrating its own database. Two stages where there used to be five. A pod stuck in
`Init` is waiting, not broken; `kubectl logs <pod> -c <init-container>` says what
for.

**A restart on a cold install is still possible, for a smaller reason than it used
to be.** The init container dials `yugabyte:5433` through a *headless* Service,
which does no readiness filtering, so it proves something is listening rather than
that the cluster is serving. A service that starts in that window fails at
`connect` and is fine on the retry. What no longer causes it is a missing database
— the service creates its own — so this is now only about the cluster being up,
not about anything having run before it. A deliberate trade against ordering every
service behind a second gate; see the note in `templates/services.yaml`.

`values-local.yaml` caps the tserver's memory. Untuned, YugabyteDB sizes itself to
the machine, and a k3d cluster shares that machine with your build. Production
leaves the caps empty and lets it have the node.

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
runs `helm upgrade --install`. Extra arguments are passed through, so
`k8s/deploy.sh prod --dry-run` works. Point `ourdriveway.local` at your ingress
controller's IP in `/etc/hosts`.

It used to carry five `--set-file` flags, one per schema, because Helm templates
cannot read files outside the chart and the `.surql` schemas lived in `schemas/`.
**Schemas are not files any more.** Each service embeds its own migrations with
`sqlx::migrate!` and applies them at boot, so a new image carries its schema with
it — that deleted the ConfigMap, the `--set-file` plumbing, and the import Job.

Every replica running migrations at boot is safe: sqlx takes an advisory lock
around the run, migrations are versioned and applied once, and each service owns
its own database so the only contention is between replicas of one service.

**Nor is `CREATE DATABASE` a central step.** `shared::db::connect` creates the
database named in `DATABASE_URL` if connecting finds none (SQLSTATE `3D000`) and
retries once, so a service brings up its own. That deleted the `create-databases`
post-install hook, the second copy of the service list inside it, and the window
where every service crash-looped waiting for a hook that runs *after* they start.
Two replicas racing is fine: the loser gets `42P04` and treats it as success.

It cannot be a migration instead. `sqlx::migrate!` runs its files on a connection
to the database being migrated, so a missing one fails at `connect` and the files
are never read — the transaction is not the obstacle, since sqlx honours a leading
`-- no-transaction`. It does assume the role in `DATABASE_URL` may create
databases, which `yugabyte` may.

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
database before it can serve — its rows are already in the cluster — so what is left to
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
services  ──►  yugabyte (StatefulSet, one database per service)
   │                    ▲
   └──►  NATS JetStream ─┘ integration events, 7 days (rebuild path)
```

**YugabyteDB is the authoritative store, and the only thing here whose loss is
data loss.** Everything else is derived from it or replaceable.

Two layers where there were four. SurrealDB stored nothing itself and needed TiKV
underneath it, which needed a placement driver underneath *that*; the schemas
needed a fifth object to apply them. One process does all of it, and the schemas
travel inside the service binaries.

Before that it was arranged the other way round entirely: every service pod
carried its own SurrealDB sidecar on an `emptyDir`, because `bus/src/projector.rs`
gave each instance an *ephemeral* consumer that replayed the whole log into a
private database and kept its cursor there — so `replicas: N` had to mean N
databases, and the log was the source of truth. None of that holds: services write
their own rows inside the request's transaction, projectors share durable
consumers, and the log is a seven-day bus.

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

It no longer costs what it used to in connections. Each lane needed its own
SurrealDB connection — 16 per projector, 64 from view-service, which runs four —
because they could not be sessions on a shared socket: `Surreal::clone` raced its
own sign-in, and a warmed clone deadlocked behind another lane's open transaction.
Both were observed.

A lane now borrows a connection from the pool for the length of one event's
transaction and gives it straight back, so 16 lanes do not mean 16 connections
held open. `max_connections` in `shared/src/db.rs` is the ceiling, and raising
`PARTITIONS` raises concurrent demand on it rather than the count directly.

Changing it needs the streams **drained** first — every `-pNN` durable at 0 pending
in `nats consumer report <STREAM>` — and then a deploy. The stream config is
reconciled on boot, but messages already stored keep the subject they were written
with, so a key that moves lanes would have its older events in one lane and its newer
ones in another, with no order between them. Drained, there is nothing left to race.
Projectors normally sit at the head, so this is a check rather than a wait.

Two consequences worth knowing:

- **`docker compose down -v` / deleting the `yugabyte` PVC is data loss**, not a
  replay. There is no log to rebuild from any more.
- **Kubernetes ≥ 1.29 is no longer required.** That floor existed for the native
  sidecar (`initContainer` + `restartPolicy: Always`) that ordered SurrealDB ahead
  of the app inside one pod. Every init container in this chart is now a plain one.

### The PVCs

| Volume | Holds | Losing it |
|---|---|---|
| `yugabyte` | every service database and the read model | **data loss** |
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

- **Single YugabyteDB node.** One copy of the authoritative data — fine for a
  demo, a data-loss risk in production. This is **not** a `replicas: 3` away, and
  the template says so: multi-node needs `--join` and a replication factor, which
  is a different template. Do that before anything real lives here, and add a
  backup story (`ysql_dump`, or YugabyteDB's own snapshots); nothing in this
  repository has ever needed one before.
- **The database is reachable from every pod in the namespace.** `YSQL_PASSWORD`
  in `app-secrets` is the only control; a NetworkPolicy admitting just the service
  pods is the next rung. Unchanged in substance from the SurrealDB arrangement —
  the credential is one key now instead of two.
- **Single NATS node.** Set `nats.config.cluster.enabled` and three replicas the
  day an outage would mean losing undelivered events rather than downtime.
- **No resource limits, only requests.** Enough to schedule sensibly, not enough
  to stop a runaway pod from starving a node.
- **The log is no longer a ledger.** `STREAM_PAYMENTS` used to keep every money
  event forever; it expires after seven days now and that record lives in the
  `payment` and `payout` tables. An *independent* audit trail, if wanted, has to be
  built as one rather than recovered by turning retention off.
