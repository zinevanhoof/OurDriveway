use std::sync::{Arc, LazyLock};

use axum::{Router, routing::get};
use shared::{
    env,
    events::{STREAM_BOOKINGS, STREAM_PAYMENTS, STREAM_SPOTS, STREAM_USERS},
};

use crate::{
    projector::{BookingProjector, PaymentProjector, SpotProjector, UserProjector},
    service::{
        account_service::AccountService, host_service::HostService, public_service::PublicService,
        renter_service::RenterService,
    },
};

mod policy;
mod projector;
mod repository;
mod route;
mod service;

bus::version_reader! {
    /// Every aggregate the read model projects — which is why this is the service where
    /// the header does the most work: a client writes to one of the four write services
    /// and then reads here, so this is the wait that actually has to happen.
    ///
    /// `payment` and `payout` are both real waits now. When view-service held `payout`
    /// but no `payment`, echoing a payment's version returned immediately; it holds both,
    /// so a client that has just paid or just withdrawn waits for the projector rather
    /// than reading a wallet without the thing it did in it.
    fn version_of;
    "user" => shared::schema::view::app_user,
    "spot" => shared::schema::view::spot,
    "booking" => shared::schema::view::booking,
    "payment" => shared::schema::view::payment,
    "payout" => shared::schema::view::payout,
}

/// The combined read model: every service's events projected into one database,
/// serving all client reads.
///
/// It owns no truth. Every table is rebuildable from the log, and nothing here is
/// ever consulted to make a decision — availability, pricing and authorization
/// are answered by the service that owns them.
///
/// **One service per namespace**, which is one per file in `route/`.
///
/// The pool is theirs rather than the state's: a handler cannot borrow a connection, so it
/// cannot take two for reads that have to agree — see `AccountService::wallet`. They share
/// no rules with each other beyond that, because a namespace *is* a predicate and these
/// are four different ones.
#[derive(Clone)]
pub struct AppState {
    pub account_service: Arc<AccountService>,
    pub host_service: Arc<HostService>,
    pub renter_service: Arc<RenterService>,
    pub public_service: Arc<PublicService>,
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
    /// How long after a booking ends its payment counts as settled — and therefore as
    /// withdrawable rather than pending.
    ///
    /// **payment-service reads the same variable and the two must agree.** This one
    /// decides what a host is *shown*; payment-service's decides what
    /// `POST /api/payment/payout` will actually hand over. Set them apart and a wallet
    /// offers a balance the withdraw endpoint refuses, or hides one it would have paid.
    /// `k8s/chart/values.yaml` sets both from one entry; the two `.env` files are on
    /// their honour.
    pub settlement_secs: i64,
}

static CONFIG: LazyLock<Config> = LazyLock::new(|| Config {
    database_url: env::require("DATABASE_URL"),
    nats_url: env::require("NATS_URL"),
    port: env::require_parsed("PORT"),
    jwt_secret: env::require("JWT_SECRET"),
    settlement_secs: env::require_parsed("SETTLEMENT_SECS"),
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
    // Schema is NOT applied here. `apps/migrator` is the only thing that migrates —
    // one Compose one-shot in dev, one Helm hook Job in production — because
    // diesel_migrations takes no lock around a run and `replicas: N` would race.
    // This process assumes its database exists and is current, and fails at connect
    // above if it does not.

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
    // repository function — one per namespace, each selecting the columns that namespace
    // may hold and matching the rows it may see. `migrations/view/0001_init/up.sql` records
    // the rules and the two problems that came with the old arrangement (a denied field
    // nulling a whole GraphQL array, and VULN-001's indexed-equality oracle).
    //
    // Ten endpoints in four namespaces, one predicate each — see `route/mod.rs`. The
    // namespace is the authorization, so a route says who may read it in the same place
    // it says what it returns, and each namespace's reads are one service in `service/`.
    //
    // Two of them are money, and they are here rather than on payment-service because
    // reads are this service's job: `/account/wallet` is the history, `/host/balance` is
    // what `GET /api/payment/earnings` used to answer.
    let app = Router::new()
        .nest(
            "/api/view/account",
            Router::new()
                .route("/", get(route::account::account))
                .route("/wallet", get(route::account::wallet)),
        )
        .nest(
            "/api/view/host",
            Router::new()
                .route("/spots", get(route::host::spots))
                .route("/spots/{id}", get(route::host::spot))
                .route("/balance", get(route::host::balance)),
        )
        .nest(
            "/api/view/renter",
            Router::new()
                // `/next` before `/{id}`: axum matches the literal segment first either
                // way, but the ordering is what a reader checks.
                .route("/bookings", get(route::renter::bookings))
                .route("/bookings/next", get(route::renter::next))
                .route("/bookings/{id}", get(route::renter::booking)),
        )
        .nest(
            "/api/view/public",
            Router::new()
                .route("/spots/nearby", get(route::public::nearby))
                .route("/spots/{id}", get(route::public::spot)),
        )
        // Waits on the aggregate versions a client echoes back, against this
        // service's own database — see `bus::await_version`. Transport-level, so it is
        // unaffected by the reads underneath it changing shape.
        .layer(axum::middleware::from_fn_with_state(
            bus::AwaitVersions(await_db, version_of),
            bus::await_version::await_version,
        ))
        .merge(bus::health::routes(readiness))
        .with_state(AppState {
            account_service: Arc::new(AccountService { db: db.clone() }),
            host_service: Arc::new(HostService { db: db.clone() }),
            renter_service: Arc::new(RenterService { db: db.clone() }),
            public_service: Arc::new(PublicService { db }),
        });

    // PORT differs per service in local dev so several can run on one host.
    // Containerised, every service listens on 80.
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", CONFIG.port)).await?;
    tracing::info!(port = CONFIG.port, "view-service listening");
    Ok(axum::serve(listener, app).await?)
}
