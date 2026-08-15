use std::sync::{Arc, LazyLock};

use axum::{
    Router,
    routing::{get, patch, post},
};
use shared::{env, rpc::spot::SUBJECT_SPOT_CARD};
use uuid::Uuid;

use crate::{
    projector::SpotProjector, repository::spot_repository::SpotRepository,
    service::spot_service::SpotService,
};

mod projector;
mod repository;
mod route;
mod service;

/// `SpotService` is built once at boot, not per request. It used to be assembled
/// inside the `DbAuthenticated` extractor on every call, purely so the connection
/// could be re-authenticated with the caller's JWT — which is exactly the pattern
/// `AuthedJwt` + a database-level user replaced.
#[derive(Clone)]
pub struct AppState {
    pub spot_service: Arc<SpotService>,
}

/// Every variable this service reads, in one place.
///
/// One-to-one with `apps/services/spot-service/.env`: if a variable is not a
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
    pub locationiq_api_key: String,
    /// Where listing photos are served from, e.g. `https://images.ourdriveway.com`.
    ///
    /// Read only to VALIDATE: the images a host sends back must be URLs
    /// media-service minted on this origin, or a listing could point its photos at
    /// any host on the internet. Must be byte-identical to media-service's and
    /// user-service's MEDIA_BASE — see `shared::media`.
    pub media_base: String,
}

pub static CONFIG: LazyLock<Config> = LazyLock::new(|| Config {
    surrealdb_addr: env::require("SURREALDB_ADDR"),
    surrealdb_user: env::require("SURREALDB_USER"),
    surrealdb_pass: env::require("SURREALDB_PASS"),
    nats_url: env::require("NATS_URL"),
    port: env::require_parsed("PORT"),
    snapshot_interval_secs: env::require_parsed("SNAPSHOT_INTERVAL_SECS"),
    jwt_secret: env::require("JWT_SECRET"),
    locationiq_api_key: env::require("LOCATIONIQ_API_KEY"),
    media_base: env::require("MEDIA_BASE"),
});

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::fmt::init();
    shared::install_default_crypto_provider();

    // Local dev only: in a container the environment comes from the orchestrator
    // and this file does not exist, so the failure is discarded. Read explicitly by
    // path so the variables resolve regardless of the process CWD.
    dotenvy::from_filename("apps/services/spot-service/.env").ok();

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
    )
    .await?;

    let js = bus::connect(&CONFIG.nats_url).await?;
    bus::ensure_streams(&js).await?;
    let readiness = bus::Readiness::new(js.client().clone(), &[shared::events::STREAM_SPOTS]);

    // No-op when SNAPSHOT_INTERVAL_SECS=0, which is how this runs with a
    // disposable projection store: every start replays from sequence 1.
    bus::snapshot::install(
        &js,
        bus::SnapshotConfig {
            db_addr: &CONFIG.surrealdb_addr,
            db_user: &CONFIG.surrealdb_user,
            db_pass: &CONFIG.surrealdb_pass,
            service: "spot-service",
            streams: vec![shared::events::STREAM_SPOTS],
            every_secs: CONFIG.snapshot_interval_secs,
        },
    )
    .await?;

    // The projector is the only *writer* to `db`. The service shares the handle to
    // read a spot's owner and shard before it publishes an edit — it still writes
    // nothing, so the dual-write the split avoids stays avoided.
    let repository = Arc::new(SpotRepository { db });

    tokio::spawn(bus::projector::run(
        js.clone(),
        Arc::new(SpotProjector {
            repository: repository.clone(),
        }),
        readiness.clone(),
    ));

    // Answers "what does spot 019fa… look like" for anyone who needs to *label* a spot
    // without becoming a consumer of SPOTS — payment-service, putting a title on a
    // Checkout Session. Read-only and unauthenticated by design: it exposes nothing a
    // renter looking at a listing cannot already see, and the connection it arrives on is
    // authenticated to NATS, which is more than an internal HTTP route would have been.
    //
    // Not gated on `readiness`. A replica still replaying answers from whatever it has
    // projected so far, which for a spot created long ago is the whole truth, and for one
    // created seconds ago is a missing title on a line item. Waiting would trade that for
    // no answer at all.
    {
        let repository = repository.clone();
        tokio::spawn(bus::service::serve(
            js.client().clone(),
            SUBJECT_SPOT_CARD,
            "spot-service",
            move |spot_id: Uuid| {
                let repository = repository.clone();
                // `images` is already absolute — stored that way, so nothing is
                // resolved here. This used to join MEDIA_BASE onto each key at the
                // edge, purely because the caller hands them to Stripe.
                async move { repository.card(&spot_id).await.ok().flatten() }
            },
        ));
    }

    let state = AppState {
        spot_service: Arc::new(SpotService { js, repository }),
    };

    let api_router: Router<AppState> = Router::new()
        .route("/api/spot", post(route::spot::create_spot))
        .route("/api/spot/address/suggest", get(route::address::suggest))
        .route(
            "/api/spot/{id}",
            patch(route::spot::update_spot).delete(route::spot::delete_spot),
        )
        .route("/api/spot/{id}/active", post(route::spot::set_active));
    // No upload route and no body limit any more: photos go straight from the
    // browser to R2 against a presigned URL, and the per-image ceiling is signed
    // into that URL by media-service. Nothing image-sized reaches this service.

    // No GraphQL proxy here any more: every client read is served by
    // view-service from the combined projection. This database is private to
    // this service — no browser identity can reach it at all.

    let app = Router::new()
        .merge(api_router)
        .merge(bus::health::routes(readiness))
        .with_state(state);

    // PORT differs per service in local dev so several can run on one host — which
    // is also how you test that independent projections converge on the same log.
    // Containerised, every service listens on 80.
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", CONFIG.port)).await?;
    tracing::info!(port = CONFIG.port, "spot-service listening");
    Ok(axum::serve(listener, app).await?)
}

#[cfg(test)]
mod tests {
    use axum::{
        Router,
        routing::{get, patch, post},
    };

    /// `/api/spot/{id}` sits alongside a static `/api/spot/address/suggest`.
    /// Overlapping paths panic when the router is *built*, not when one is
    /// requested — so without this the failure mode is a service that dies on boot
    /// in whatever environment ran it first.
    ///
    /// Dummy handlers on purpose: the panic comes from the path set alone, and the
    /// real ones need a live NATS connection to construct.
    #[test]
    fn route_paths_do_not_overlap() {
        let _: Router = Router::new()
            .route("/api/spot", post(|| async {}))
            .route("/api/spot/address/suggest", get(|| async {}))
            .route("/api/spot/{id}", patch(|| async {}).delete(|| async {}))
            .route("/api/spot/{id}/active", post(|| async {}));
    }
}
