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

use crate::{
    await_seq::AppliedSeqs,
    projector::{BookingProjector, PaymentProjector, SpotProjector, UserProjector},
    repository::ViewRepository,
};

mod await_seq;
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
    pub repository: Arc<ViewRepository>,
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
    let repository = Arc::new(ViewRepository { db });

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

    tokio::spawn(bus::projector::run(
        js.clone(),
        Arc::new(UserProjector {
            repository: repository.clone(),
        }),
        readiness.clone(),
    ));
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
    tokio::spawn(bus::projector::run(
        js,
        Arc::new(PaymentProjector {
            repository: repository.clone(),
        }),
        readiness.clone(),
    ));

    // Reads reach SurrealDB with the client's own Authorization header forwarded
    // verbatim, so they get a RECORD identity and are constrained by the table
    // permissions in view-schema.surql. The OWNER connection above is never
    // exposed here.
    let proxy: Router<AppState> =
        ReverseProxy::new(
            "/api/view/graphql",
            &format!("http://{}/graphql", CONFIG.surrealdb_addr),
        )
        .into();

    let app = Router::new()
        .route("/api/view/me", get(route::me::me))
        .merge(proxy)
        .layer(axum::middleware::from_fn_with_state(
            applied,
            await_seq::await_seq,
        ))
        .merge(bus::health::routes(readiness))
        .with_state(AppState { repository });

    // PORT differs per service in local dev so several can run on one host.
    // Containerised, every service listens on 80.
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", CONFIG.port)).await?;
    tracing::info!(port = CONFIG.port, "view-service listening");
    Ok(axum::serve(listener, app).await?)
}
