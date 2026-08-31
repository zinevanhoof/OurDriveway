use std::sync::{Arc, LazyLock};

use axum::{Router, routing::get};
use shared::{
    env,
    events::{STREAM_BOOKINGS, STREAM_PAYMENTS, STREAM_SPOTS, STREAM_USERS},
};

use crate::projector::{BookingProjector, PaymentProjector, SpotProjector, UserProjector};

mod policy;
mod projector;
mod repository;
mod route;

/// The combined read model: every service's events projected into one database,
/// serving all client reads.
///
/// It owns no truth. Every table is rebuildable from the log, and nothing here is
/// ever consulted to make a decision — availability, pricing and authorization
/// are answered by the service that owns them.
#[derive(Clone)]
pub struct AppState {
    /// The pool. Handlers read through the repositories, which are stateless.
    pub db: sqlx::PgPool,
}

/// Every variable this service reads, in one place.
///
/// One-to-one with `apps/services/view-service/.env`: if a variable is not a
/// field here it is not read, and if it is a field here it is required. Nothing
/// falls back to a default, because a default is a value you cannot discover by
/// reading the `.env`.
pub struct Config {
    /// This service's own database in the YugabyteDB cluster, as one URL — and the
    /// port is **5433**, not 5432. See user-service's `Config` for the full note.
    pub database_url: String,
    pub nats_url: String,
    pub port: u16,
    /// Verification only. This service mints no tokens; user-service does.
    pub jwt_secret: String,
}

static CONFIG: LazyLock<Config> = LazyLock::new(|| Config {
    database_url: env::require("DATABASE_URL"),
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

    let db = shared::db::connect(&CONFIG.database_url).await?;
    shared::db::migrate(&db, &sqlx::migrate!("../../../migrations/view")).await?;

    // One pool for the whole process — four projectors × PARTITIONS lanes, the
    // election, the relay, the handlers and the await layer all share it.
    //
    // This used to be 4 × 16 connections plus three more, on the theory that an open
    // transaction blocks every other session on the socket. It did not, but a
    // `Surreal::clone` carried a replayed sign-in that `bus/examples/clone_cost`
    // priced at +27.5ms per transaction. A pool has neither problem: a lane borrows a
    // connection for one event's transaction and gives it straight back.
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

    // The GraphQL proxy is gone, and with it the security model it carried.
    //
    // `/api/view/graphql` reverse-proxied straight to SurrealDB with the client's own
    // Authorization header forwarded verbatim, so a browser got a RECORD identity and
    // was constrained by the `PERMISSIONS` clauses in view-schema.surql. Those clauses
    // *were* the authorization, evaluated by the database against `$auth`.
    //
    // No browser reaches this database now, and every rule those clauses expressed is a
    // repository function — one per audience, each selecting the columns that audience
    // may hold and matching the rows it may see. `migrations/view/0001_init.sql` records
    // the rules and the two problems that came with the old arrangement (a denied field
    // nulling a whole GraphQL array, and VULN-001's indexed-equality oracle).
    //
    // Eight endpoints, replacing eleven GraphQL documents. One audience, one projection
    // and one repository call each — see `route/mod.rs` for the rules that shape them.
    let app = Router::new()
        .route("/api/view/me", get(route::me::me))
        .route("/api/view/me/spots", get(route::me::spots))
        .route("/api/view/me/bookings", get(route::me::bookings))
        .route("/api/view/me/payouts", get(route::me::payouts))
        .route("/api/view/spots/nearby", get(route::spot::nearby))
        .route("/api/view/spots/{id}", get(route::spot::public))
        .route("/api/view/spots/{id}/manage", get(route::spot::manage))
        .route("/api/view/bookings/{id}", get(route::booking::detail))
        // Waits on the aggregate versions a client echoes back, against this
        // service's own database — see `bus::await_version`. Transport-level, so it is
        // unaffected by the reads underneath it changing shape.
        .layer(axum::middleware::from_fn_with_state(
            bus::AwaitVersions(await_db),
            bus::await_version::await_version,
        ))
        .merge(bus::health::routes(readiness))
        .with_state(AppState { db });

    // PORT differs per service in local dev so several can run on one host.
    // Containerised, every service listens on 80.
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", CONFIG.port)).await?;
    tracing::info!(port = CONFIG.port, "view-service listening");
    Ok(axum::serve(listener, app).await?)
}
