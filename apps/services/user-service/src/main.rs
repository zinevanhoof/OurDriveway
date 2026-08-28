use std::sync::{Arc, LazyLock};

use axum::{
    Router,
    routing::{patch, post},
};
use shared::env;

use crate::{
    repository::{
        refresh_token_repository::RefreshTokenRepository, user_repository::UserRepository,
    },
    service::{refresh_token_service::RefreshTokenService, user_service::UserService},
};

#[derive(Clone)]
pub struct AppState {
    pub user_service: Arc<UserService>,
    pub refresh_token_service: Arc<RefreshTokenService>,
}

/// Every variable this service reads, in one place.
///
/// One-to-one with `apps/services/user-service/.env`: if a variable is not a
/// field here it is not read, and if it is a field here it is required. Nothing
/// falls back to a default, because a default is a value you cannot discover by
/// reading the `.env`.
pub struct Config {
    /// Where profile pictures are served from. Read only to VALIDATE: the avatar a
    /// client sends back must be a URL media-service minted on this origin, or a
    /// user could point their picture at any host. Same value as media-service's
    /// MEDIA_BASE and spot-service's — see `shared::media`.
    pub media_base: String,
    pub surrealdb_addr: String,
    pub surrealdb_user: String,
    pub surrealdb_pass: String,
    /// This service's own database inside the shared `main` namespace. Every
    /// service used to be "main"; on TiKV they share one keyspace, so this is
    /// what keeps their tables apart.
    pub surrealdb_db: String,
    pub nats_url: String,
    pub port: u16,
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
    media_base: env::require("MEDIA_BASE"),
    surrealdb_addr: env::require("SURREALDB_ADDR"),
    surrealdb_user: env::require("SURREALDB_USER"),
    surrealdb_pass: env::require("SURREALDB_PASS"),
    surrealdb_db: env::require("SURREALDB_DB"),
    nats_url: env::require("NATS_URL"),
    port: env::require_parsed("PORT"),
    jwt_secret: env::require("JWT_SECRET"),
    email_token_secret: env::require("EMAIL_TOKEN_SECRET"),
    jwt_expiration: env::require_parsed("JWT_EXPIRATION"),
    refresh_token_expiration: env::require_parsed("REFRESH_TOKEN_EXPIRATION"),
});

mod auth;
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
    // Installs the origin `shared::media` mints and validates against. Beside the
    // CONFIG force for the same reason: a missing base must stop the process, not
    // surface as a rejected upload later.
    shared::media::init_base(&CONFIG.media_base);
    shared::init_jwt_decoding_key(&CONFIG.jwt_secret);

    let db = shared::db::connect(
        &CONFIG.surrealdb_addr,
        &CONFIG.surrealdb_user,
        &CONFIG.surrealdb_pass,
        &CONFIG.surrealdb_db,
    )
    .await?;

    // One connection for the whole process — election, relay, handlers and the await
    // layer all share it. `Surreal::clone` would mint a session and replay the root
    // sign-in onto it; cloning the `Arc` is a refcount bump. The sessions that do get
    // minted are per *transaction*, in `shared::db::begin`, and die with it.
    let db = Arc::new(db);
    let await_db = db.clone();

    let js = bus::connect(&CONFIG.nats_url).await?;
    bus::ensure_streams(&js).await?;
    // No streams: this service projects nothing now. It writes its own rows
    // directly, so "am I caught up" has no meaning here and `/readyz` reduces to
    // "is NATS reachable" — which still matters, because the outbox relay needs it.
    let readiness = bus::Readiness::new(js.client().clone(), &[]);



    // For the outbox relay, which is now the only thing here that must run on
    // exactly one instance. There are no projectors left to elect for: this
    // service writes its own rows inside the request's transaction, and the event
    // goes into `_outbox` in that same transaction.
    let leader = bus::lease::elect(db.clone(), bus::lease::instance_id());

    // Carries every USERS and SESSIONS event this service commits. No longer
    // scaffolding — this is the only path by which those events reach NATS.
    tokio::spawn(bus::outbox::run(db.clone(), js.clone(), leader.clone()));


    let state = AppState {
        user_service: Arc::new(UserService {
            users: UserRepository { q: db.clone() },
        }),
        refresh_token_service: Arc::new(RefreshTokenService {
            tokens: RefreshTokenRepository { q: db },
        }),
    };

    let api_router = Router::new()
        .route("/api/user/login", post(route::login::login))
        .route("/api/user/signup", post(route::signup::signup))
        // Both under `auth::cookie::SESSION_PATH`, which is what the refresh-token
        // cookie is scoped to — that scoping is the reason they share a prefix
        // rather than sitting beside `login`. See `auth::cookie`.
        .route("/api/user/session/refresh", post(route::refresh::refresh))
        .route("/api/user/session/logout", post(route::logout::logout))
        // Both unauthenticated: the token in the link is the credential, and a
        // user who cannot log in yet is exactly who needs these.
        .route("/api/user/email/verify", post(route::email::verify))
        .route("/api/user/email/resend", post(route::email::resend))
        // The authenticated user themselves — which one is the JWT's business, so
        // there is no id in the path and nothing to scope under.
        // PATCH is also the change-password form: same record, and the service
        // decides from the body which event that becomes.
        .route("/api/user", patch(route::user::update_user));

    // No GraphQL proxy here any more: every client read is served by
    // view-service from the combined projection. This database is private to
    // this service — no browser identity can reach it at all.

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
        // off the ingress; see `route::user::backfill`.
        .route("/internal/backfill", post(route::user::backfill))
        .merge(bus::health::routes(readiness))
        .with_state(state);

    // PORT differs per service in local dev so several can run on one host — which
    // is also how you test that independent projections converge on the same log.
    // Containerised, every service listens on 80 and is told apart by its Service.
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", CONFIG.port)).await?;
    tracing::info!(port = CONFIG.port, "user-service listening");
    Ok(axum::serve(listener, app).await?)
}
