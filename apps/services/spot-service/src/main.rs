use std::sync::{Arc, LazyLock};

use axum::{Router, extract::DefaultBodyLimit, routing::{get, post}};
use tower_http::services::ServeDir;

use crate::{
    projector::SpotProjector, repository::spot_repository::SpotRepository,
    service::spot_service::SpotService,
};

mod projector;
mod repository;
mod route;
mod service;

/// `SpotService` is built once at boot, not per request. It used to be assembled
/// inside the `DbAuthenticated` extractor on every call, purely so the connection
/// could be re-authenticated with the caller's JWT — which is exactly the pattern
/// `AuthedJwt` + a database-level user replaced.
#[derive(Clone)]
pub struct AppState {
    pub spot_service: Arc<SpotService>,
}

pub struct Config {
    pub locationiq_api_key: String,
}

pub static CONFIG: LazyLock<Config> = LazyLock::new(|| Config {
    locationiq_api_key: std::env::var("LOCATIONIQ_API_KEY")
        .expect("LOCATIONIQ_API_KEY must be set"),
});

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::fmt::init();
    shared::install_default_crypto_provider();

    // Read spot-service's own .env explicitly so the key resolves regardless of the
    // process CWD (the workspace has several services).
    dotenvy::from_filename("apps/services/spot-service/.env").ok();
    shared::check_config();

    let db_addr = std::env::var("SURREALDB_ADDR").unwrap_or_else(|_| "localhost:8002".into());
    let db = shared::db::connect(&db_addr).await?;

    let js = bus::connect().await?;
    bus::ensure_streams(&js).await?;
    let readiness = bus::Readiness::new(js.client().clone(), &[shared::events::STREAM_SPOTS]);

    // Cold start only: restore the projection from the newest snapshot before the
    // projectors begin, so replay resumes from the snapshot's cursor instead of
    // sequence 1. A warm restart finds a non-empty projection and skips this.
    let snapshotter = std::sync::Arc::new(
        bus::snapshot::connect(&js, &db_addr, "spot-service", vec![shared::events::STREAM_SPOTS]).await?,
    );
    snapshotter.restore_if_empty().await?;
    bus::snapshot::spawn(snapshotter, bus::snapshot::interval_from_env());

    // The projector is the only writer to `db`; the service only publishes.
    tokio::spawn(bus::projector::run(
        js.clone(),
        Arc::new(SpotProjector {
            repository: SpotRepository { db },
        }),
        readiness.clone(),
    ));

    let state = AppState {
        spot_service: Arc::new(SpotService { js }),
    };

    let api_router: Router<AppState> = Router::new()
        .route("/api/spot", post(route::spot::create_spot))
        .route("/api/spot/address/suggest", get(route::address::suggest))
        .nest_service("/api/spot/uploads", ServeDir::new("uploads"))
        .layer(DefaultBodyLimit::disable());

    // No GraphQL proxy here any more: every client read is served by
    // view-service from the combined projection. This database is private to
    // this service — no browser identity can reach it at all.

    let app = Router::new()
        .merge(api_router)
        .merge(bus::health::routes(readiness))
        .with_state(state);

    // run our app with hyper, listening globally on port 3000
    // PORT is overridable so several instances can run on one host — needed to
    // test that independent projections converge on the same log.
    let port = std::env::var("PORT").unwrap_or_else(|_| "3002".into());
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await.unwrap();
    Ok(axum::serve(listener, app).await.unwrap())
}
