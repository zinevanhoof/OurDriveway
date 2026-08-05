# Deploying OurDriveway on Kubernetes

Plain Helm. No Kustomize — `kubectl kustomize` / `kubectl apply -k` uses kubectl's
embedded copy, which omits `--enable-helm` and `--load-restrictor`, so it can
neither inflate a chart nor read `schemas/` from outside `k8s/`. Helm covers
everything we wanted Kustomize for anyway: `--set-file` crosses the chart
boundary, values files replace overlays, and a `checksum/` annotation replaces the
generator hash.

```
k8s/
  chart/              the chart — four backends are data in values.yaml,
                      one template in templates/services.yaml
  values-local.yaml   dev cluster: no TLS, 1 replica each
  values-prod.yaml    TLS, 2 replicas (spot stays at 1 — see below)
  deploy.sh
  secrets.env.example
```

## Once

```sh
helm repo add nats https://nats-io.github.io/k8s/helm/charts/
helm dependency update k8s/chart          # vendors the NATS subchart
cp k8s/secrets.env.example k8s/secrets.env && $EDITOR k8s/secrets.env
```

Kubernetes must be **≥ 1.29** — every service pod uses a native sidecar for
startup ordering, and `shared::db::connect` does not retry.

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
for i in user-service booking-service spot-service view-service frontend; do
  k3d image import ghcr.io/zinevanhoof/ourdriveway-$i:latest -c ourdriveway
done

# 3. Deploy, and resolve the host.
k8s/deploy.sh local
echo "127.0.0.1 ourdriveway.local" | sudo tee -a /etc/hosts
```

Then http://ourdriveway.local. `kubectl get pods -n ourdriveway -w` while it
comes up: each pod sits in `Init` until NATS accepts connections, then starts
SurrealDB, replays every stream from sequence 1, and only then passes `/readyz`.
No restarts on a cold cluster — the `wait-for-nats` init container is this
chart's `depends_on`, so a pod that boots ahead of the broker waits instead of
crash-looping.

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
runs `helm upgrade --install` with the four `--set-file` flags that carry the
schemas in. Extra arguments are passed through, so `k8s/deploy.sh prod --dry-run`
works. Point `ourdriveway.local` at your ingress controller's IP in `/etc/hosts`.

The Secret is created by kubectl rather than templated, because Helm stores every
value it renders in the release secret and hands them back to anyone who runs
`helm get values` — no place for a signing key. The cost is that rotating it
needs an explicit `kubectl rollout restart deployment -n ourdriveway`.

## What load-balances what

Nothing in Caddy any more. A ClusterIP Service sits in front of each service's
pods and kube-proxy spreads connections across the ready ones, so scaling is
`replicas: N` in a values file and nothing else. `readinessProbe: /readyz` keeps a
pod that is still replaying the log out of that endpoint list — the job the old
`health_uri /readyz` checks in the Caddyfile were doing, done a layer lower where
it actually works.

The Ingress owns all routing: `/api/*` to the four services, everything else to
Caddy. One host, so the browser stays on one origin and the `SameSite=Strict`
refresh cookie keeps working.

## Why every pod carries its own database

`bus/src/projector.rs` creates an **ephemeral** consumer per instance and keeps
its cursor in the projection database. That is fan-out, not work-sharing: every
replica replays the whole stream into its own store. Two replicas sharing one
database means two projectors racing over one cursor.

So SurrealDB is a sidecar in each pod, bound to `127.0.0.1`, running the
`surrealkv` engine on an `emptyDir` — a real on-disk datastore that simply gets
deleted with the pod. Not `memory`, which would hold the whole read model in RAM.
`replicas: N` therefore gives N databases for free, and there is no PVC, no
StatefulSet, and nothing to back up or migrate.

`SNAPSHOT_INTERVAL_SECS=0` turns off snapshot/restore to match. Every pod start
replays from sequence 1.

## The one PVC

NATS. It is the source of truth; everything else is a projection of it. Losing
that volume is the only way to lose data in this deployment.

## Known ceilings

- **Uploaded images die with the spot-service pod** (`emptyDir`), and spots keep
  URLs that then 404. This is the only state NATS cannot rebuild. It is also why
  `spot-service` is pinned to one replica. Fix by writing images to the NATS
  object store (already a dependency) or S3.
- **Single NATS node.** Set `nats.config.cluster.enabled` and three replicas the
  day an outage would mean data loss rather than downtime.
- **Cold starts get slower as the log grows.** When that is felt, set
  `snapshotIntervalSecs` back to `900` and give the snapshotter an object store.
