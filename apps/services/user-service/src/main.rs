use std::sync::{Arc, LazyLock};

use axum::{
    Router,
    routing::{patch, post},
};
use shared::env;

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

/// Every variable this service reads, in one place.
///
/// One-to-one with `apps/services/user-service/.env`: if a variable is not a
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
    pub jwt_secret: String,
    /// Verification links only, and deliberately NOT `jwt_secret`. `JwtClaims`
    /// carries no purpose or audience field, so a link signed with the access
    /// token key would be accepted by `AuthedJwt` as a full session — see the
    /// test in `shared::email_token`.
    pub email_token_secret: String,
    /// Minutes.
    pub jwt_expiration: i64,
    /// Days.
    pub refresh_token_expiration: i64,
}

static CONFIG: LazyLock<Config> = LazyLock::new(|| Config {
    surrealdb_addr: env::require("SURREALDB_ADDR"),
    surrealdb_user: env::require("SURREALDB_USER"),
    surrealdb_pass: env::require("SURREALDB_PASS"),
    nats_url: env::require("NATS_URL"),
    port: env::require_parsed("PORT"),
    snapshot_interval_secs: env::require_parsed("SNAPSHOT_INTERVAL_SECS"),
    jwt_secret: env::require("JWT_SECRET"),
    email_token_secret: env::require("EMAIL_TOKEN_SECRET"),
    jwt_expiration: env::require_parsed("JWT_EXPIRATION"),
    refresh_token_expiration: env::require_parsed("REFRESH_TOKEN_EXPIRATION"),
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

    // Local dev only: in a container the environment comes from the orchestrator
    // and this file does not exist, so the failure is discarded. Read explicitly by
    // path so the variables resolve regardless of the process CWD.
    dotenvy::from_filename("apps/services/user-service/.env").ok();

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

    let js = bus::connect(&CONFIG.nats_url).await?;
    bus::ensure_streams(&js).await?;
    let readiness = bus::Readiness::new(
        js.client().clone(),
        &[
            shared::events::STREAM_USERS,
            shared::events::STREAM_SESSIONS,
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
            service: "user-service",
            streams: vec![
                shared::events::STREAM_USERS,
                shared::events::STREAM_SESSIONS,
            ],
            every_secs: CONFIG.snapshot_interval_secs,
        },
    )
    .await?;

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
        .route("/api/user/refresh", post(route::refresh::refresh))
        // Both unauthenticated: the token in the link is the credential, and a
        // user who cannot log in yet is exactly who needs these.
        .route(
            "/api/user/verify-email",
            post(route::verify::verify_email),
        )
        .route(
            "/api/user/verify-email/resend",
            post(route::verify::resend_verification),
        )
        .route("/api/user/me", patch(route::profile::update_profile))
        .route(
            "/api/user/me/password",
            post(route::profile::change_password),
        );

    // No GraphQL proxy here any more: every client read is served by
    // view-service from the combined projection. This database is private to
    // this service — no browser identity can reach it at all.

    let app = Router::new()
        .merge(api_router)
        .merge(bus::health::routes(readiness))
        .with_state(state);

    // PORT differs per service in local dev so several can run on one host — which
    // is also how you test that independent projections converge on the same log.
    // Containerised, every service listens on 80 and is told apart by its Service.
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", CONFIG.port)).await?;
    tracing::info!(port = CONFIG.port, "user-service listening");
    Ok(axum::serve(listener, app).await?)
}
