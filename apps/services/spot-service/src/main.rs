use std::sync::LazyLock;

use axum::{
    Router,
    extract::{DefaultBodyLimit, FromRef},
    routing::{get, post},
};
use axum_reverse_proxy::ReverseProxy;
use surrealdb::{
    Surreal,
    engine::remote::ws::{Client, Ws},
};
use tower_http::services::ServeDir;

mod extractors;
mod repository;
mod route;
mod service;

#[derive(Clone, FromRef)]
pub struct AppState {
    pub db: Surreal<Client>,
}

pub struct Config {
    pub locationiq_api_key: String,
}

pub static CONFIG: LazyLock<Config> = LazyLock::new(|| Config {
    locationiq_api_key: std::env::var("LOCATIONIQ_API_KEY")
        .expect("LOCATIONIQ_API_KEY must be set"),
});

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();
    shared::install_default_crypto_provider();

    // Read spot-service's own .env explicitly so the key resolves regardless of the
    // process CWD (the workspace has several services).
    dotenvy::from_filename("apps/services/spot-service/.env").ok();

    let db_addr = std::env::var("SURREALDB_ADDR").unwrap_or_else(|_| "localhost:8002".into());
    let db = Surreal::new::<Ws>(&db_addr).await?;

    db.use_ns("main").use_db("main").await?;

    let state = AppState { db };

    let api_router: Router<AppState> = Router::new()
        .route("/api/spot", post(route::spot::test_spot))
        .route("/api/spot/address/suggest", get(route::address::suggest))
        .nest_service("/api/spot/uploads", ServeDir::new("uploads"))
        .layer(DefaultBodyLimit::disable());

    let proxy: Router<AppState> =
        ReverseProxy::new("/api/spot/graphql", &format!("http://{db_addr}/graphql")).into();

    let app = Router::new()
        .merge(api_router)
        .merge(proxy)
        .with_state(state);

    // run our app with hyper, listening globally on port 3000
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3002").await.unwrap();
    Ok(axum::serve(listener, app).await.unwrap())
}
