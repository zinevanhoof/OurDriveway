use std::sync::Arc;

use axum::{
    Router,
    routing::{delete, post},
};

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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::fmt::init();
    shared::install_default_crypto_provider();
    // Read booking-service's own .env explicitly so the key resolves regardless
    // of the process CWD (the workspace has several services).
    dotenvy::from_filename("apps/services/booking-service/.env").ok();
    shared::check_config();

    let db_addr = std::env::var("SURREALDB_ADDR").unwrap_or_else(|_| "localhost:8001".into());
    let db = shared::db::connect(&db_addr).await?;
    let repository = Arc::new(BookingRepository { db });

    let js = bus::connect().await?;
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

    // Cold start only: restore the projection from the newest snapshot before the
    // projectors begin, so replay resumes from the snapshot's cursor instead of
    // sequence 1. A warm restart finds a non-empty projection and skips this.
    let snapshotter = std::sync::Arc::new(
        bus::snapshot::connect(
            &js,
            &db_addr,
            "booking-service",
            vec![
                shared::events::STREAM_SPOTS,
                shared::events::STREAM_BOOKINGS,
            ],
        )
        .await?,
    );
    snapshotter.restore_if_empty().await?;
    bus::snapshot::spawn(snapshotter, bus::snapshot::interval_from_env());

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

    // PORT is overridable so several instances can run on one host — needed to
    // test that two instances racing the same slot produce exactly one winner.
    let port = std::env::var("PORT").unwrap_or_else(|_| "3001".into());
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await?;
    tracing::info!(%port, "booking-service listening");
    Ok(axum::serve(listener, app).await?)
}
