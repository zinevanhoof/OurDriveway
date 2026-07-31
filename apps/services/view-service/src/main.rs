use std::{collections::HashMap, sync::Arc};

use axum::{Router, routing::get};
use axum_reverse_proxy::ReverseProxy;
use shared::events::{STREAM_SPOTS, STREAM_USERS};

use crate::{
    await_seq::AppliedSeqs,
    projector::{SpotProjector, UserProjector},
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::fmt::init();
    shared::install_default_crypto_provider();

    dotenvy::from_filename("apps/services/view-service/.env").ok();
    shared::check_config();

    let db_addr = std::env::var("SURREALDB_ADDR").unwrap_or_else(|_| "localhost:8003".into());
    let db = shared::db::connect(&db_addr).await?;
    let repository = Arc::new(ViewRepository { db });

    let js = bus::connect().await?;
    bus::ensure_streams(&js).await?;
    let readiness = bus::Readiness::new(js.client().clone(), &[STREAM_USERS, STREAM_SPOTS]);

    // Cold start only: restore the projection from the newest snapshot before the
    // projectors begin, so replay resumes from the snapshot's cursor instead of
    // sequence 1. A warm restart finds a non-empty projection and skips this.
    let snapshotter = std::sync::Arc::new(
        bus::snapshot::connect(&js, &db_addr, "view-service", vec![STREAM_USERS, STREAM_SPOTS]).await?,
    );
    snapshotter.restore_if_empty().await?;
    bus::snapshot::spawn(snapshotter, bus::snapshot::interval_from_env());

    let applied = AppliedSeqs(Arc::new(HashMap::from([
        (STREAM_USERS, readiness.applied_rx(STREAM_USERS).unwrap()),
        (STREAM_SPOTS, readiness.applied_rx(STREAM_SPOTS).unwrap()),
    ])));

    tokio::spawn(bus::projector::run(
        js.clone(),
        Arc::new(UserProjector {
            repository: repository.clone(),
        }),
        readiness.clone(),
    ));
    tokio::spawn(bus::projector::run(
        js,
        Arc::new(SpotProjector {
            repository: repository.clone(),
        }),
        readiness.clone(),
    ));

    // Reads reach SurrealDB with the client's own Authorization header forwarded
    // verbatim, so they get a RECORD identity and are constrained by the table
    // permissions in view-schema.surql. The OWNER connection above is never
    // exposed here.
    let proxy: Router<AppState> =
        ReverseProxy::new("/api/view/graphql", &format!("http://{db_addr}/graphql")).into();

    let app = Router::new()
        .route("/api/view/me", get(route::me::me))
        .merge(proxy)
        .layer(axum::middleware::from_fn_with_state(
            applied,
            await_seq::await_seq,
        ))
        .merge(bus::health::routes(readiness))
        .with_state(AppState { repository });

    let port = std::env::var("PORT").unwrap_or_else(|_| "3003".into());
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await?;
    tracing::info!(%port, "view-service listening");
    Ok(axum::serve(listener, app).await?)
}
