use std::sync::{Arc, LazyLock};

use axum::{Router, routing::post};

use crate::{
    projector::{SessionProjector, UserProjector},
    repository::{
        refresh_token_repository::RefreshTokenRepository, user_repository::UserRepository,
    },
    service::user_service::UserService,
};

#[derive(Clone)]
pub struct AppState {
    pub user_service: Arc<UserService>,
}

pub struct Config {
    pub jwt_secret: String,
    pub jwt_expiration: i64,
    pub refresh_token_expiration: i64,
}

static CONFIG: LazyLock<Config> = LazyLock::new(|| Config {
    jwt_secret: std::env::var("JWT_SECRET").expect("JWT_SECRET must be set"),
    jwt_expiration: std::env::var("JWT_EXPIRATION")
        .expect("JWT_EXPIRATION must be set")
        .parse()
        .unwrap(),
    refresh_token_expiration: std::env::var("REFRESH_TOKEN_EXPIRATION")
        .expect("REFRESH_TOKEN_EXPIRATION must be set")
        .parse()
        .unwrap(),
});

mod auth;
mod projector;
mod repository;
mod route;
mod service;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::fmt::init();
    shared::install_default_crypto_provider();

    // Read user-service's own .env explicitly so the key resolves regardless of the
    // process CWD (the workspace has several services).
    dotenvy::from_filename("apps/services/user-service/.env").ok();
    shared::check_config();

    let db_addr = std::env::var("SURREALDB_ADDR").unwrap_or_else(|_| "localhost:8000".into());
    let db = shared::db::connect(&db_addr).await?;

    let js = bus::connect().await?;
    bus::ensure_streams(&js).await?;
    let readiness = bus::Readiness::new(
        js.client().clone(),
        &[
            shared::events::STREAM_USERS,
            shared::events::STREAM_SESSIONS,
        ],
    );

    // Cold start only: restore the projection from the newest snapshot before the
    // projectors begin, so replay resumes from the snapshot's cursor instead of
    // sequence 1. A warm restart finds a non-empty projection and skips this.
    let snapshotter = std::sync::Arc::new(
        bus::snapshot::connect(&js, &db_addr, "user-service", vec![shared::events::STREAM_USERS, shared::events::STREAM_SESSIONS]).await?,
    );
    snapshotter.restore_if_empty().await?;
    bus::snapshot::spawn(snapshotter, bus::snapshot::interval_from_env());
    let users_applied = readiness
        .applied_rx(shared::events::STREAM_USERS)
        .expect("USERS registered above");
    let sessions_applied = readiness
        .applied_rx(shared::events::STREAM_SESSIONS)
        .expect("SESSIONS registered above");

    // Two projectors, two streams, two independent consumers.
    tokio::spawn(bus::projector::run(
        js.clone(),
        Arc::new(UserProjector {
            repository: UserRepository { db: db.clone() },
        }),
        readiness.clone(),
    ));
    tokio::spawn(bus::projector::run(
        js.clone(),
        Arc::new(SessionProjector {
            repository: RefreshTokenRepository { db: db.clone() },
        }),
        readiness.clone(),
    ));

    let state = AppState {
        user_service: Arc::new(UserService {
            user_repository: UserRepository { db: db.clone() },
            refresh_token_repository: RefreshTokenRepository { db },
            js,
            users_applied,
            sessions_applied,
        }),
    };

    let api_router = Router::new()
        .route("/api/user/login", post(route::login::login))
        .route("/api/user/signup", post(route::signup::signup))
        .route("/api/user/refresh/logout", post(route::logout::logout))
        .route("/api/user/refresh", post(route::refresh::refresh));

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
    let port = std::env::var("PORT").unwrap_or_else(|_| "3000".into());
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await.unwrap();
    Ok(axum::serve(listener, app).await.unwrap())
}
