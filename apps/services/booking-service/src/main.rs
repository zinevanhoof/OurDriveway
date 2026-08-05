use std::sync::{Arc, LazyLock};

use axum::{
    Router,
    routing::{delete, post},
};
use shared::env;

use crate::{
    projector::{BookingProjector, SpotProjector},
    repository::booking_repository::BookingRepository,
    service::booking_service::BookingService,
};

mod projector;
mod repository;
mod route;
mod service;

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
    pub surrealdb_addr: String,
    pub surrealdb_user: String,
    pub surrealdb_pass: String,
    pub nats_url: String,
    pub port: u16,
    /// 0 disables snapshots entirely — see `bus::snapshot::install`.
    pub snapshot_interval_secs: u64,
    /// Verification only. This service mints no tokens; user-service does.
    pub jwt_secret: String,
}

static CONFIG: LazyLock<Config> = LazyLock::new(|| Config {
    surrealdb_addr: env::require("SURREALDB_ADDR"),
    surrealdb_user: env::require("SURREALDB_USER"),
    surrealdb_pass: env::require("SURREALDB_PASS"),
    nats_url: env::require("NATS_URL"),
    port: env::require_parsed("PORT"),
    snapshot_interval_secs: env::require_parsed("SNAPSHOT_INTERVAL_SECS"),
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

    let db = shared::db::connect(
        &CONFIG.surrealdb_addr,
        &CONFIG.surrealdb_user,
        &CONFIG.surrealdb_pass,
    )
    .await?;
    let repository = Arc::new(BookingRepository { db });

    let js = bus::connect(&CONFIG.nats_url).await?;
    bus::ensure_streams(&js).await?;
    // SPOTS as well as BOOKINGS: this service needs a local projection of spot
    // price and availability to authorize and price a booking server-side, since
    // the spot table lives in another service's database.
    let readiness = bus::Readiness::new(
        js.client().clone(),
        &[
            shared::events::STREAM_SPOTS,
            shared::events::STREAM_BOOKINGS,
        ],
    );

    // No-op when SNAPSHOT_INTERVAL_SECS=0, which is how this runs with a
    // disposable projection store: every start replays from sequence 1.
    bus::snapshot::install(
        &js,
        bus::SnapshotConfig {
            db_addr: &CONFIG.surrealdb_addr,
            db_user: &CONFIG.surrealdb_user,
            db_pass: &CONFIG.surrealdb_pass,
            service: "booking-service",
            streams: vec![
                shared::events::STREAM_SPOTS,
                shared::events::STREAM_BOOKINGS,
            ],
            every_secs: CONFIG.snapshot_interval_secs,
        },
    )
    .await?;

    // The projectors are the only writers to `db`; the service only publishes.
    // No `mark_caught_up` short-circuit any more — /readyz must stay 503 until
    // these have actually replayed, or Caddy routes bookings at an instance whose
    // availability projection is still half-built.
    tokio::spawn(bus::projector::run(
        js.clone(),
        Arc::new(SpotProjector {
            repository: repository.clone(),
        }),
        readiness.clone(),
    ));
    tokio::spawn(bus::projector::run(
        js.clone(),
        Arc::new(BookingProjector {
            repository: repository.clone(),
        }),
        readiness.clone(),
    ));

    // Nothing else frees a lapsed hold — `spot.booked` carries no expiry, so no
    // reader can filter one out. See service/expiry.rs.
    service::expiry::spawn(js.clone(), repository.clone());

    let state = AppState {
        booking_service: Arc::new(BookingService::new(js, repository, &readiness)),
    };

    // No GraphQL proxy here: every client read is served by view-service from the
    // combined projection. This database is private to this service — no browser
    // identity can reach it at all.
    let api_router: Router<AppState> = Router::new()
        .route("/api/booking", post(route::booking::reserve))
        .route("/api/booking/{id}/confirm", post(route::booking::confirm))
        .route("/api/booking/{id}", delete(route::booking::release));

    let app = Router::new()
        .merge(api_router)
        .merge(bus::health::routes(readiness))
        .with_state(state);

    // PORT differs per service in local dev so several can run on one host — which
    // is also how you test that two instances racing the same slot produce exactly
    // one winner. Containerised, every service listens on 80.
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", CONFIG.port)).await?;
    tracing::info!(port = CONFIG.port, "booking-service listening");
    Ok(axum::serve(listener, app).await?)
}
