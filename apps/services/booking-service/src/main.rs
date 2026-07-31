use axum::Router;
use surrealdb::{Surreal, engine::remote::ws::Client};

#[derive(Clone)]
pub struct AppState {
    pub db: Surreal<Client>,
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

    let _state = AppState { db };

    let js = bus::connect().await?;
    bus::ensure_streams(&js).await?;
    // booking-service will consume SPOTS too — it needs a local projection of spot
    // price and availability to compute an amount server-side, since the spot table
    // lives in another service's database.
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
        bus::snapshot::connect(&js, &db_addr, "booking-service", vec![shared::events::STREAM_SPOTS, shared::events::STREAM_BOOKINGS]).await?,
    );
    snapshotter.restore_if_empty().await?;
    bus::snapshot::spawn(snapshotter, bus::snapshot::interval_from_env());
    // Nothing publishes or projects yet; the booking write path is a later phase.
    readiness.mark_caught_up(shared::events::STREAM_SPOTS);
    readiness.mark_caught_up(shared::events::STREAM_BOOKINGS);

    // No GraphQL proxy here any more: every client read is served by
    // view-service from the combined projection. This database is private to
    // this service — no browser identity can reach it at all.

    let app = Router::new().merge(bus::health::routes(readiness));

    // run our app with hyper, listening globally on port 3000
    // PORT is overridable so several instances can run on one host — needed to
    // test that independent projections converge on the same log.
    let port = std::env::var("PORT").unwrap_or_else(|_| "3001".into());
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await.unwrap();
    Ok(axum::serve(listener, app).await.unwrap())
}
