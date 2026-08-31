use std::sync::{Arc, LazyLock};

use axum::{
    Router,
    routing::{delete, post},
};
use shared::env;

use crate::{
    projector::SpotProjector,
    service::booking_service::BookingService,
};

mod policy;
mod projector;
mod repository;
mod route;
mod service;
mod sweeper;
mod worker;

#[derive(Clone)]
pub struct AppState {
    pub booking_service: Arc<BookingService>,
}

/// Every variable this service reads, in one place.
///
/// One-to-one with `apps/services/booking-service/.env`: if a variable is not a
/// field here it is not read, and if it is a field here it is required. Nothing
/// falls back to a default, because a default is a value you cannot discover by
/// reading the `.env`.
pub struct Config {
    /// This service's own database in the YugabyteDB cluster, as one URL — and the
    /// port is **5433**, not 5432. See user-service's `Config` for the full note.
    pub database_url: String,
    pub nats_url: String,
    pub port: u16,
    /// Verification only. This service mints no tokens; user-service does.
    pub jwt_secret: String,
}

static CONFIG: LazyLock<Config> = LazyLock::new(|| Config {
    database_url: env::require("DATABASE_URL"),
    nats_url: env::require("NATS_URL"),
    port: env::require_parsed("PORT"),
    jwt_secret: env::require("JWT_SECRET"),
});

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::fmt::init();
    shared::install_default_crypto_provider();
    // Local dev only: in a container the environment comes from the orchestrator
    // and this file does not exist, so the failure is discarded. Read explicitly by
    // path so the variables resolve regardless of the process CWD.
    dotenvy::from_filename("apps/services/booking-service/.env").ok();

    // Resolve the whole environment before anything binds a port. Without this a
    // missing variable would surface as a panic inside the first handler that
    // needed it, leaving a process that passes its health check and fails requests.
    LazyLock::force(&CONFIG);
    shared::init_jwt_decoding_key(&CONFIG.jwt_secret);

    let db = shared::db::connect(&CONFIG.database_url).await?;
    shared::db::migrate(&db, &sqlx::migrate!("../../../migrations/booking")).await?;

    // One pool for the whole process — projector lanes, election, relay, sweeper,
    // handlers and the await layer all share it. `PgPool` is `Arc` inside, so a clone
    // is a refcount bump; a connection is borrowed per statement or per transaction.
    let await_db = db.clone();

    let js = bus::connect(&CONFIG.nats_url).await?;
    bus::ensure_streams(&js).await?;
    // SPOTS as well as BOOKINGS: this service needs a local projection of spot
    // price and availability to authorize and price a booking server-side, since
    // the spot table lives in another service's database.
    // SPOTS only. BOOKINGS is this service's own stream and it no longer projects
    // it — those rows are written directly by the request that causes them.
    let readiness = bus::Readiness::new(js.client().clone(), &[shared::events::STREAM_SPOTS]);

    // The service writes `db` directly, inside each request's transaction.
    // No `mark_caught_up` short-circuit any more — /readyz must stay 503 until
    // these have actually replayed, or Caddy routes bookings at an instance whose
    // availability projection is still half-built.
    // `run` opens a transaction per event, applies and commits — so the projector
    // below cannot forget any of that, and `react`'s publishes sit inside the same
    // transaction as the projection write they precede.
    // For the outbox relay only. The projectors need no election: each partition is
    // one durable consumer with `max_ack_pending: 1`, so JetStream hands out one
    // event at a time *per partition* across every replica, in order — and different
    // partitions are different spots, which have no order between them. The relay has
    // no such backstop, so exactly one instance may run it.
    let leader = bus::lease::elect(db.clone(), bus::lease::instance_id());

    tokio::spawn(bus::projector::run(
        js.clone(),
        // A host's edit can invalidate bookings, and withdrawing them is this
        // stream's job — see `SpotProjector::react`. It writes them and enqueues
        // their events in the same transaction, so it needs no NATS handle.
        //
        // Partitioning is what makes that safe to run concurrently: SPOTS is keyed
        // by spot, so one spot's edits stay in one lane and `react` can never race
        // itself over the same spot's bookings.
        Arc::new(SpotProjector),
        db.clone(),
        readiness.clone(),
    ));

    // Carries every BOOKINGS event this service commits — the only path by which
    // they reach NATS.
    tokio::spawn(bus::outbox::run(db.clone(), js.clone(), leader.clone()));

    // Nothing else frees a lapsed hold — a `reserved` row blocks regardless of its
    // `hold_until`, by design. See sweeper.rs.
    tokio::spawn(sweeper::run(db.clone()));

    let booking_service = Arc::new(BookingService::new(db.clone()));

    // Payment confirms bookings. A worker rather than a projector, and this service
    // keeps no PAYMENTS projection — see worker.rs. Deliberately not registered with
    // `Readiness`: it builds nothing, so there is nothing for /readyz to wait on, and
    // listing PAYMENTS there would hold the instance at 503 until a stream that may be
    // empty had been "replayed".
    //
    // Its own service rather than the `BookingService` above: the worker path is
    // post-capture and shares none of the request path's rules. See
    // service/payment_worker_service.rs.
    let service = Arc::new(service::payment_worker_service::PaymentWorkerService::new(db));
    tokio::spawn(bus::worker::run(
        js,
        Arc::new(worker::PaymentWorker { service }),
    ));

    let state = AppState { booking_service };

    // No GraphQL proxy here: every client read is served by view-service from the
    // combined projection. This database is private to this service — no browser
    // identity can reach it at all.
    let api_router: Router<AppState> = Router::new()
        .route("/api/booking", post(route::booking::create_booking))
        .route("/api/booking/{id}", delete(route::booking::release))
        .route("/api/booking/{id}/cancel", post(route::booking::cancel));


    let app = Router::new()
        .merge(api_router)
        // On the API only, and before health is merged: `/readyz` reporting how far
        // behind a projector is must never itself wait for that projector.
        // Waits on the aggregate versions a client echoes back, against this
        // service's own database — see `bus::await_version`.
        .layer(axum::middleware::from_fn_with_state(
            bus::AwaitVersions(await_db),
            bus::await_version::await_version,
        ))
        // After the layer, deliberately — a backfill is not a client read and has
        // no version to wait on. Not under `/api` either, which is what keeps it
        // off the ingress; see `route::booking::backfill`.
        .route("/internal/backfill", post(route::booking::backfill))
        .merge(bus::health::routes(readiness))
        .with_state(state);

    // PORT differs per service in local dev so several can run on one host — which
    // is also how you test that two instances racing the same slot produce exactly
    // one winner. Containerised, every service listens on 80.
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", CONFIG.port)).await?;
    tracing::info!(port = CONFIG.port, "booking-service listening");
    Ok(axum::serve(listener, app).await?)
}
