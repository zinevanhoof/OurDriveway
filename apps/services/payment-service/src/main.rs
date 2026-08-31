use std::sync::{Arc, LazyLock};

use axum::{
    Router,
    routing::{get, post},
};
use shared::{
    env,
    events::STREAM_BOOKINGS,
};

use crate::{
    client::stripe::Stripe,
    projector::BookingProjector,
    service::{
        payment_service::PaymentService, settlement_worker_service::SettlementWorkerService,
    },
    worker::{BookingWorker, PaymentWorker},
};

mod client;
mod policy;
mod projector;
mod repository;
mod route;
mod service;
mod worker;

#[derive(Clone)]
pub struct AppState {
    pub payment_service: Arc<PaymentService>,
}

/// Every variable this service reads, in one place.
///
/// One-to-one with `apps/services/payment-service/.env`: if a variable is not a field
/// here it is not read, and if it is a field here it is required. Nothing falls back to
/// a default, because a default is a value you cannot discover by reading the `.env`.
pub struct Config {
    /// This service's own database in the YugabyteDB cluster, as one URL — and the
    /// port is **5433**, not 5432. See user-service's `Config` for the full note.
    pub database_url: String,
    pub nats_url: String,
    pub port: u16,
    /// Verification only. This service mints no tokens; user-service does.
    pub jwt_secret: String,
    /// `sk_test_…` for a sandbox. Never logged, and the only credential that can move
    /// money.
    pub stripe_secret_key: String,
    /// `whsec_…`. In dev this is the CLI session's own secret
    /// (`stripe listen --print-secret`), which is *not* the same value as a dashboard
    /// endpoint's in production.
    pub stripe_webhook_secret: String,
    /// How long after a booking ends its money becomes withdrawable by the host.
    ///
    /// The one knob on the earnings rule. A renter cannot cancel inside an hour of the
    /// start and the host-side withdrawal only touches bookings that haven't ended, so
    /// `confirmed` plus a past `ends_at` is already safe today — this exists so that
    /// stays true if those rules are ever relaxed, and because paying out the instant a
    /// car drives off is not how anyone else does it. Set to 0 locally to test a payout
    /// without waiting a day for a booking to age.
    pub settlement_secs: i64,
}

static CONFIG: LazyLock<Config> = LazyLock::new(|| Config {
    database_url: env::require("DATABASE_URL"),
    nats_url: env::require("NATS_URL"),
    port: env::require_parsed("PORT"),
    jwt_secret: env::require("JWT_SECRET"),
    stripe_secret_key: env::require("STRIPE_SECRET_KEY"),
    stripe_webhook_secret: env::require("STRIPE_WEBHOOK_SECRET"),
    settlement_secs: env::require_parsed("SETTLEMENT_SECS"),
});

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::fmt::init();
    shared::install_default_crypto_provider();
    // Local dev only: in a container the environment comes from the orchestrator and
    // this file does not exist, so the failure is discarded. Read explicitly by path so
    // the variables resolve regardless of the process CWD.
    dotenvy::from_filename("apps/services/payment-service/.env").ok();

    // Resolve the whole environment before anything binds a port, so a missing Stripe
    // key crashes at startup rather than surfacing as a 500 in the middle of checkout.
    LazyLock::force(&CONFIG);
    shared::init_jwt_decoding_key(&CONFIG.jwt_secret);

    let db = shared::db::connect(&CONFIG.database_url).await?;
    shared::db::migrate(&db, &sqlx::migrate!("../../../migrations/payment")).await?;

    // One pool for the whole process — projector lanes, election, relay, workers,
    // handlers and the await layer all share it. `PgPool` is `Arc` inside, so a clone
    // is a refcount bump; a connection is borrowed per statement or per transaction.
    let await_db = db.clone();

    let js = bus::connect(&CONFIG.nats_url).await?;
    bus::ensure_streams(&js).await?;

    // BOOKINGS as well as its own stream: this service needs a booking's price, renter
    // and lifecycle to authorize a payment and to decide a refund, and the booking
    // table lives in another service's database.
    // BOOKINGS only now. PAYMENTS is this service's own stream and it no longer
    // projects it — those rows are written directly by the request that causes them.
    let readiness = bus::Readiness::new(js.client().clone(), &[STREAM_BOOKINGS]);


    let stripe = Arc::new(Stripe::new(&CONFIG.stripe_secret_key));
    let settlement = Arc::new(SettlementWorkerService {
        db: db.clone(),
        stripe: stripe.clone(),
    });

    // The service writes `db` directly, inside each request's transaction. `run`
    // opens a transaction per event, applies and commits.
    // For the outbox relay only. The projectors need no election: each partition is
    // one durable consumer with `max_ack_pending: 1`, so JetStream hands out one
    // event at a time *per partition* across every replica, in order — and different
    // partitions are different bookings, which have no order between them. The relay
    // has no such backstop, so exactly one instance may run it.
    let leader = bus::lease::elect(db.clone(), bus::lease::instance_id());

    tokio::spawn(bus::projector::run(
        js.clone(),
        Arc::new(BookingProjector),
        db.clone(),
        readiness.clone(),
    ));

    // Carries every PAYMENTS event this service commits — the only path by which
    // they reach NATS.
    tokio::spawn(bus::outbox::run(db.clone(), js.clone(), leader.clone()));

    // The refund side. Workers, not projectors — one refund per event across the whole
    // deployment, and no replay of history on a cold start.
    //
    // Deliberately NOT partitioned. A worker performs a side effect and needs no
    // order between events; partitioning it would cap refund throughput at
    // `PARTITIONS` for nothing.
    //
    // Only the BOOKINGS one holds a database handle, because only BOOKINGS is a
    // stream this service projects — it waits for its own mirror to include the event
    // that woke it before deciding whether to move money.
    tokio::spawn(bus::worker::run(
        js.clone(),
        Arc::new(BookingWorker {
            service: settlement.clone(),
            db: db.clone(),
        }),
    ));
    // No handle for this one: PAYMENTS is this service's own stream and it
    // projects nothing off it any more — see `PaymentWorker::handle`.
    tokio::spawn(bus::worker::run(
        js.clone(),
        Arc::new(PaymentWorker {
            service: settlement,
        }),
    ));

    let state = AppState {
        payment_service: Arc::new(PaymentService::new(js, db, stripe, CONFIG.settlement_secs)),
    };

    // No GraphQL here, and no reads of anything but this user's own money. Payout
    // *history* is served by view-service from the combined projection; this database is
    // private to this service and no browser identity can reach it at all.
    let api_router: Router<AppState> = Router::new()
        .route("/api/payment/session", post(route::payment::create_session))
        .route(
            "/api/payment/session/{session_id}",
            get(route::payment::session_state),
        )
        .route("/api/payment/earnings", get(route::payment::earnings))
        .route("/api/payment/payout", post(route::payment::request_payout))
        // Unauthenticated by design — authentication is the Stripe signature. Auth in
        // this codebase is a per-handler extractor rather than a router layer, so this
        // route simply omits it and there is no middleware exception to get wrong.
        .route("/api/payment/webhook", post(route::webhook::stripe_webhook));


    let app = Router::new()
        .merge(api_router)
        // On the API only, and before health is merged: `/readyz` reporting how far
        // behind a projector is must never itself wait for that projector.
        //
        // The Stripe webhook sits under this too, harmlessly: Stripe sends no
        // `X-Await-Version`, so the layer is a header lookup that finds nothing.
        // Waits on the aggregate versions a client echoes back, against this
        // service's own database — see `bus::await_version`.
        .layer(axum::middleware::from_fn_with_state(
            bus::AwaitVersions(await_db),
            bus::await_version::await_version,
        ))
        // After the layer, deliberately — a backfill is not a client read and has
        // no version to wait on. Not under `/api` either, which is what keeps it
        // off the ingress; see `route::payment::backfill`.
        .route("/internal/backfill", post(route::payment::backfill))
        .merge(bus::health::routes(readiness))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", CONFIG.port)).await?;
    tracing::info!(port = CONFIG.port, "payment-service listening");
    Ok(axum::serve(listener, app).await?)
}
