use std::{
    collections::HashMap,
    sync::{Arc, LazyLock},
};

use axum::{Router, routing::get};
use axum_reverse_proxy::ReverseProxy;
use shared::{
    env,
    events::{STREAM_BOOKINGS, STREAM_PAYMENTS, STREAM_SPOTS, STREAM_USERS},
};

use bus::AppliedSeqs;

use crate::{
    projector::{BookingProjector, PaymentProjector, SpotProjector, UserProjector},
    repository::user_repository::ViewUserRepository,
};

mod projector;
mod repository;
mod route;

/// The combined read model: every service's events projected into one database
/// with real record links, serving all client reads from a single endpoint.
///
/// It owns no truth. Every table is rebuildable from the log, and nothing here is
/// ever consulted to make a decision — availability, pricing and authorization
/// are answered by the service that owns them.
#[derive(Clone)]
pub struct AppState {
    /// Only the `user` table, and only for `/me`. Everything else a client reads
    /// comes through the GraphQL proxy below, straight from the database.
    pub users: Arc<ViewUserRepository>,
}

/// Every variable this service reads, in one place.
///
/// One-to-one with `apps/services/view-service/.env`: if a variable is not a
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
    dotenvy::from_filename("apps/services/view-service/.env").ok();

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

    // One owned client per projector — four here, because `Surreal::begin` consumes
    // one and each holds its own open transaction. That is the floor: four sessions,
    // cloned once at boot rather than once per event.
    let users_client = db.clone();
    let spots_client = db.clone();
    let bookings_client = db.clone();
    let payments_client = db.clone();

    // The `/me` handler shares one session. `Surreal::clone` would mint another and
    // replay the root sign-in onto it; cloning the `Arc` is a refcount bump. Safe
    // because nothing re-authenticates per request — see `shared::db::connect`.
    let db = Arc::new(db);

    let js = bus::connect(&CONFIG.nats_url).await?;
    bus::ensure_streams(&js).await?;
    let readiness = bus::Readiness::new(
        js.client().clone(),
        &[STREAM_USERS, STREAM_SPOTS, STREAM_BOOKINGS, STREAM_PAYMENTS],
    );

    // No-op when SNAPSHOT_INTERVAL_SECS=0, which is how this runs with a
    // disposable projection store: every start replays from sequence 1.
    bus::snapshot::install(
        &js,
        bus::SnapshotConfig {
            db_addr: &CONFIG.surrealdb_addr,
            db_user: &CONFIG.surrealdb_user,
            db_pass: &CONFIG.surrealdb_pass,
            service: "view-service",
            streams: vec![STREAM_USERS, STREAM_SPOTS, STREAM_BOOKINGS, STREAM_PAYMENTS],
            every_secs: CONFIG.snapshot_interval_secs,
        },
    )
    .await?;

    let applied = AppliedSeqs(Arc::new(HashMap::from([
        (STREAM_USERS, readiness.applied_rx(STREAM_USERS).unwrap()),
        (STREAM_SPOTS, readiness.applied_rx(STREAM_SPOTS).unwrap()),
        (
            STREAM_BOOKINGS,
            readiness.applied_rx(STREAM_BOOKINGS).unwrap(),
        ),
        (
            STREAM_PAYMENTS,
            readiness.applied_rx(STREAM_PAYMENTS).unwrap(),
        ),
    ])));

    // `Tx` opens a transaction per event, applies, advances that stream's cursor
    // inside it and commits — so none of the four below can forget any of it.
    tokio::spawn(bus::projector::run(
        js.clone(),
        bus::Tx::new(UserProjector, users_client),
        readiness.clone(),
    ));
    tokio::spawn(bus::projector::run(
        js.clone(),
        bus::Tx::new(SpotProjector, spots_client),
        readiness.clone(),
    ));
    tokio::spawn(bus::projector::run(
        js.clone(),
        bus::Tx::new(BookingProjector, bookings_client),
        readiness.clone(),
    ));
    tokio::spawn(bus::projector::run(
        js,
        bus::Tx::new(PaymentProjector, payments_client),
        readiness.clone(),
    ));

    // Reads reach SurrealDB with the client's own Authorization header forwarded
    // verbatim, so they get a RECORD identity and are constrained by the table
    // permissions in view-schema.surql. The OWNER connection above is never
    // exposed here.
    let proxy: Router<AppState> = ReverseProxy::new(
        "/api/view/graphql",
        &format!("http://{}/graphql", CONFIG.surrealdb_addr),
    )
    .into();

    let app = Router::new()
        .route("/api/view/me", get(route::me::me))
        .merge(proxy)
        .layer(axum::middleware::from_fn_with_state(
            applied,
            bus::await_seq::await_seq,
        ))
        .merge(bus::health::routes(readiness))
        .with_state(AppState {
            users: Arc::new(ViewUserRepository { q: db }),
        });

    // PORT differs per service in local dev so several can run on one host.
    // Containerised, every service listens on 80.
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", CONFIG.port)).await?;
    tracing::info!(port = CONFIG.port, "view-service listening");
    Ok(axum::serve(listener, app).await?)
}
