use std::sync::{Arc, LazyLock};

use axum::{Router, routing::get};
use axum_reverse_proxy::ReverseProxy;
use shared::{
    env,
    events::{STREAM_BOOKINGS, STREAM_PAYMENTS, STREAM_SPOTS, STREAM_USERS},
};


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
    /// This service's own database inside the shared `main` namespace. Every
    /// service used to be "main"; on TiKV they share one keyspace, so this is
    /// what keeps their tables apart.
    pub surrealdb_db: String,
    pub nats_url: String,
    pub port: u16,
    /// Verification only. This service mints no tokens; user-service does.
    pub jwt_secret: String,
}

static CONFIG: LazyLock<Config> = LazyLock::new(|| Config {
    surrealdb_addr: env::require("SURREALDB_ADDR"),
    surrealdb_user: env::require("SURREALDB_USER"),
    surrealdb_pass: env::require("SURREALDB_PASS"),
    surrealdb_db: env::require("SURREALDB_DB"),
    nats_url: env::require("NATS_URL"),
    port: env::require_parsed("PORT"),
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
        &CONFIG.surrealdb_db,
    )
    .await?;

    // One connection for the whole process — projectors, election, relay, handlers
    // and the await layer all share it. `Surreal::clone` would mint a session and
    // replay the root sign-in onto it; cloning the `Arc` is a refcount bump. The
    // sessions that do get minted are per *transaction*, in `shared::db::begin`, and
    // die with it.
    //
    // This used to be 4 x 16 connections for the projectors plus three more for the
    // lease, the relay and the await layer, on the theory that an open transaction
    // blocks every other session on the socket. It does not — but the replayed sign-in
    // a clone carries is not free either, and `bus/examples/clone_cost` prices it at
    // +27.5ms per transaction against a connection of one's own.
    let db = Arc::new(db);
    let await_db = db.clone();

    let js = bus::connect(&CONFIG.nats_url).await?;
    bus::ensure_streams(&js).await?;
    let readiness = bus::Readiness::new(
        js.client().clone(),
        &[STREAM_USERS, STREAM_SPOTS, STREAM_BOOKINGS, STREAM_PAYMENTS],
    );



    // For the outbox relay only. The projectors need no election: each partition is
    // one durable consumer with `max_ack_pending: 1`, so JetStream hands out one
    // event at a time *per partition* across every replica, in order — and different
    // partitions are different aggregates, which have no order between them. The
    // relay has no such backstop, so exactly one instance may run it.
    let leader = bus::lease::elect(db.clone(), bus::lease::instance_id());

    // Four projectors, `PARTITIONS` lanes each, all on the one connection above.
    // A transaction per event, on a session that lives only as long as it does.
    tokio::spawn(bus::projector::run(
        js.clone(),
        Arc::new(UserProjector),
        db.clone(),
        readiness.clone(),
    ));
    tokio::spawn(bus::projector::run(
        js.clone(),
        Arc::new(SpotProjector),
        db.clone(),
        readiness.clone(),
    ));
    tokio::spawn(bus::projector::run(
        js.clone(),
        Arc::new(BookingProjector),
        db.clone(),
        readiness.clone(),
    ));
    tokio::spawn(bus::projector::run(
        js.clone(),
        Arc::new(PaymentProjector),
        db.clone(),
        readiness.clone(),
    ));

    // view-service publishes nothing today, so this relay has nothing to carry.
    // Spawned anyway so every service has the same shape and a future event from
    // the read model has somewhere to go.
    tokio::spawn(bus::outbox::run(db.clone(), js, leader.clone()));

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
        // Waits on the aggregate versions a client echoes back, against this
        // service's own database — see `bus::await_version`.
        .layer(axum::middleware::from_fn_with_state(
            bus::AwaitVersions(await_db),
            bus::await_version::await_version,
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
