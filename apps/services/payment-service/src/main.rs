use std::sync::{Arc, LazyLock};

use axum::{
    Router,
    routing::{get, post},
};
use shared::{
    env,
    events::{STREAM_BOOKINGS, STREAM_PAYMENTS},
};

use crate::{
    projector::{BookingProjector, PaymentProjector},
    repository::payment_repository::PaymentRepository,
    service::{payment_service::PaymentService, settle::Settler, stripe::Stripe},
    worker::{BookingWorker, PaymentWorker},
};

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
    pub surrealdb_addr: String,
    pub surrealdb_user: String,
    pub surrealdb_pass: String,
    pub nats_url: String,
    pub port: u16,
    /// 0 disables snapshots entirely — see `bus::snapshot::install`.
    pub snapshot_interval_secs: u64,
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
    surrealdb_addr: env::require("SURREALDB_ADDR"),
    surrealdb_user: env::require("SURREALDB_USER"),
    surrealdb_pass: env::require("SURREALDB_PASS"),
    nats_url: env::require("NATS_URL"),
    port: env::require_parsed("PORT"),
    snapshot_interval_secs: env::require_parsed("SNAPSHOT_INTERVAL_SECS"),
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

    let db = shared::db::connect(
        &CONFIG.surrealdb_addr,
        &CONFIG.surrealdb_user,
        &CONFIG.surrealdb_pass,
    )
    .await?;
    let repository = Arc::new(PaymentRepository { db });

    let js = bus::connect(&CONFIG.nats_url).await?;
    bus::ensure_streams(&js).await?;

    // BOOKINGS as well as its own stream: this service needs a booking's price, renter
    // and lifecycle to authorize a payment and to decide a refund, and the booking
    // table lives in another service's database.
    let readiness = bus::Readiness::new(js.client().clone(), &[STREAM_BOOKINGS, STREAM_PAYMENTS]);

    bus::snapshot::install(
        &js,
        bus::SnapshotConfig {
            db_addr: &CONFIG.surrealdb_addr,
            db_user: &CONFIG.surrealdb_user,
            db_pass: &CONFIG.surrealdb_pass,
            service: "payment-service",
            streams: vec![STREAM_BOOKINGS, STREAM_PAYMENTS],
            every_secs: CONFIG.snapshot_interval_secs,
        },
    )
    .await?;

    let stripe = Arc::new(Stripe::new(&CONFIG.stripe_secret_key));
    let settler = Arc::new(Settler {
        repository: repository.clone(),
        stripe: stripe.clone(),
        js: js.clone(),
    });

    // The projectors are the only writers to `db`; the service only publishes.
    tokio::spawn(bus::projector::run(
        js.clone(),
        Arc::new(BookingProjector {
            repository: repository.clone(),
        }),
        readiness.clone(),
    ));
    tokio::spawn(bus::projector::run(
        js.clone(),
        Arc::new(PaymentProjector {
            repository: repository.clone(),
        }),
        readiness.clone(),
    ));

    // The refund side. Workers, not projectors — one refund per event across the whole
    // deployment, and no replay of history on a cold start. Each holds its own stream's
    // applied cursor so it can decide against a projection that includes the event that
    // woke it. `.expect` is safe: both streams were just named to `Readiness::new`.
    tokio::spawn(bus::worker::run(
        js.clone(),
        Arc::new(BookingWorker {
            settler: settler.clone(),
            applied: readiness
                .applied_rx(STREAM_BOOKINGS)
                .expect("BOOKINGS is registered with Readiness above"),
        }),
    ));
    tokio::spawn(bus::worker::run(
        js.clone(),
        Arc::new(PaymentWorker {
            settler: settler.clone(),
            applied: readiness
                .applied_rx(STREAM_PAYMENTS)
                .expect("PAYMENTS is registered with Readiness above"),
        }),
    ));

    let state = AppState {
        payment_service: Arc::new(PaymentService::new(
            js,
            repository,
            stripe,
            settler,
            CONFIG.settlement_secs,
        )),
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
        .merge(bus::health::routes(readiness))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", CONFIG.port)).await?;
    tracing::info!(port = CONFIG.port, "payment-service listening");
    Ok(axum::serve(listener, app).await?)
}
