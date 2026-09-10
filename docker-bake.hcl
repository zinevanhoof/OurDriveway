// Builds every image in ONE BuildKit run.
//
// Each service has its own Dockerfile, but none of them compile anything: they
// pull their binary from the `builder` target via a named build context
// (`target:builder`). Because bake resolves all targets in a single graph, that
// builder is materialised once and the Rust workspace compiles once — four
// separate `docker build` invocations would be four graphs and four compiles.
//
//   docker buildx bake --load             # local, tags :local
//   TAG=abc123 docker buildx bake --push   # CI
// Matches the `${TAG:-latest}` default in docker-compose-prod.yml, so a local
// `bake --load` followed by a plain `docker compose up` lines up with no env var.
// CI overrides this with the commit sha; the promote step moves :latest to it.
variable "TAG" {
  default = "latest"
}

variable "REGISTRY" {
  default = "ghcr.io/zinevanhoof"
}

// `builder` is deliberately absent: it carries no tags and is never pushed, it
// only exists to be consumed by the service targets.
group "default" {
  targets = ["user-service", "booking-service", "spot-service", "view-service", "media-service", "notification-service", "payment-service", "migrator", "frontend"]
}

target "builder" {
  dockerfile = "Dockerfile.builder"
}

// Not a service, and deliberately not named like one: it runs to completion and exits.
// It is the only thing that migrates — no service does, because diesel_migrations takes
// no lock around a run and `replicas: N` would race. See diesel-migration.md.
target "migrator" {
  dockerfile = "apps/migrator/Dockerfile"
  contexts   = { builder = "target:builder" }
  tags       = ["${REGISTRY}/ourdriveway-migrator:${TAG}"]
}

target "user-service" {
  dockerfile = "apps/services/user-service/Dockerfile"
  contexts   = { builder = "target:builder" }
  tags       = ["${REGISTRY}/ourdriveway-user-service:${TAG}"]
}

target "booking-service" {
  dockerfile = "apps/services/booking-service/Dockerfile"
  contexts   = { builder = "target:builder" }
  tags       = ["${REGISTRY}/ourdriveway-booking-service:${TAG}"]
}

target "spot-service" {
  dockerfile = "apps/services/spot-service/Dockerfile"
  contexts   = { builder = "target:builder" }
  tags       = ["${REGISTRY}/ourdriveway-spot-service:${TAG}"]
}

target "view-service" {
  dockerfile = "apps/services/view-service/Dockerfile"
  contexts   = { builder = "target:builder" }
  tags       = ["${REGISTRY}/ourdriveway-view-service:${TAG}"]
}

target "media-service" {
  dockerfile = "apps/services/media-service/Dockerfile"
  contexts   = { builder = "target:builder" }
  tags       = ["${REGISTRY}/ourdriveway-media-service:${TAG}"]
}

target "notification-service" {
  dockerfile = "apps/services/notification-service/Dockerfile"
  contexts   = { builder = "target:builder" }
  tags       = ["${REGISTRY}/ourdriveway-notification-service:${TAG}"]
}

target "payment-service" {
  dockerfile = "apps/services/payment-service/Dockerfile"
  contexts   = { builder = "target:builder" }
  tags       = ["${REGISTRY}/ourdriveway-payment-service:${TAG}"]
}

// Context is still the repo root, even though everything this target needs now
// sits under apps/frontend/ — it is bake's default and the COPY paths assume it.
target "frontend" {
  dockerfile = "apps/frontend/Dockerfile"
  tags       = ["${REGISTRY}/ourdriveway-frontend:${TAG}"]
}
