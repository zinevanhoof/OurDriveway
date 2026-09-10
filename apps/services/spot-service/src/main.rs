use std::sync::{Arc, LazyLock};

use axum::{
    Router,
    routing::{get, patch, post},
};
use shared::{
    env,
    rpc::spot::{SUBJECT_SPOT_CARD, SpotCard},
};
use uuid::Uuid;

use crate::{repository::spot_repository::SpotRepository, service::spot_service::SpotService};

mod client;
mod policy;
mod repository;
mod route;
mod service;

bus::version_reader! {
    /// What this service can answer an `X-Await-Version` header about: its own `spot`
    /// rows, and nothing else. Anything else a client echoes here — a booking it just
    /// made, say — is `Unavailable`, so the request proceeds instead of waiting two
    /// seconds for a table this database does not have.
    fn version_of;
    "spot" => shared::schema::spot::spot,
}

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
    /// This service's own database in the YugabyteDB cluster, as one URL — and the
    /// port is **5433**, not 5432. See user-service's `Config` for the full note.
    pub database_url: String,
    pub nats_url: String,
    pub port: u16,
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
    database_url: env::require("DATABASE_URL"),
    nats_url: env::require("NATS_URL"),
    port: env::require_parsed("PORT"),
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

    let db = shared::db::connect(&CONFIG.database_url).await?;
    // Schema is NOT applied here. `apps/migrator` is the only thing that migrates —
    // one Compose one-shot in dev, one Helm hook Job in production — because
    // diesel_migrations takes no lock around a run and `replicas: N` would race.
    // This process assumes its database exists and is current, and fails at connect
    // above if it does not.

    // One pool for the whole process — election, relay, handlers, the card RPC and
    // the await layer all share it. `PgPool` is `Arc` inside, so a clone is a
    // refcount bump; a connection is borrowed per statement or per transaction.
    let await_db = db.clone();

    let js = bus::connect(&CONFIG.nats_url).await?;
    bus::ensure_streams(&js).await?;
    // No streams: this service projects nothing now — it writes its own rows and
    // enqueues the event beside them. `/readyz` reduces to "is NATS reachable",
    // which the outbox relay still needs.
    let readiness = bus::Readiness::new(js.client().clone(), &[]);

    // The projector is the only *writer* to `db`. The service reads a spot's host
    // before it publishes an edit — it still writes nothing, so the dual-write the
    // split avoids stays avoided.
    //
    // For the outbox relay, which is all this service runs off the bus — it has no
    // projector and no worker, so there is nothing else here an election would gate.
    // The relay has no backstop of its own, so exactly one instance may run it.
    let leader = bus::lease::elect(db.clone(), bus::lease::instance_id());

    // Carries every SPOTS event this service commits. This is now the only path by
    // which they reach NATS.
    tokio::spawn(bus::outbox::run(db.clone(), js.clone(), leader.clone()));

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
        let db = db.clone();
        tokio::spawn(bus::service::serve(
            js.client().clone(),
            SUBJECT_SPOT_CARD,
            "spot-service",
            move |spot_id: Uuid| {
                // A pool handle per call — a refcount bump. The repository itself is
                // stateless, so there is nothing else to construct.
                let db = db.clone();
                // `images` is already absolute — stored that way, so nothing is
                // resolved here. This used to join MEDIA_BASE onto each key at the
                // edge, purely because the caller hands them to Stripe.
                //
                // Narrowed to a `SpotCard` here rather than by the query: the whole
                // row is one point read either way, and a second projection shape
                // was one more thing to keep in step with the table.
                async move {
                    // A connection per RPC, released as soon as the read is done.
                    let mut conn = shared::db::conn(&db).await.ok()?;
                    SpotRepository::find_by_id(&mut conn, spot_id)
                        .await
                        .ok()
                        .flatten()
                        .map(SpotCard::from)
                }
            },
        ));
    }

    let state = AppState {
        spot_service: Arc::new(SpotService { db }),
    };

    let api_router: Router<AppState> = Router::new()
        .route("/api/spot", post(route::spot::create_spot))
        .route("/api/spot/address/suggest", get(route::address::suggest))
        .route(
            "/api/spot/{id}",
            // PATCH is also the live switch: an edit carrying nothing but `active`.
            patch(route::spot::update_spot).delete(route::spot::delete_spot),
        );
    // No upload route and no body limit any more: photos go straight from the
    // browser to R2 against a presigned URL, and the per-image ceiling is signed
    // into that URL by media-service. Nothing image-sized reaches this service.

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
            bus::AwaitVersions(await_db, version_of),
            bus::await_version::await_version,
        ))
        // After the layer, deliberately — a backfill is not a client read and has
        // no version to wait on. Not under `/api` either, which is what keeps it
        // off the ingress; see `route::spot::backfill`.
        .route("/internal/backfill", post(route::spot::backfill))
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
            .route("/api/spot/{id}", patch(|| async {}).delete(|| async {}));
    }
}
