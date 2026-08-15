use std::sync::{Arc, LazyLock};

use aws_sdk_s3::config::{BehaviorVersion, Credentials, Region};
use axum::{
    Router,
    http::StatusCode,
    routing::{get, post},
};
use shared::env;

mod route;

/// Everything this service reads, in one place.
///
/// One-to-one with `apps/services/media-service/.env`: if a variable is not a
/// field here it is not read, and if it is a field here it is required. Nothing
/// falls back to a default, because a default is a value you cannot discover by
/// reading the `.env`.
pub struct Config {
    pub port: u16,
    /// Verification only. This service mints no tokens; user-service does.
    pub jwt_secret: String,
    /// `https://<account>.r2.cloudflarestorage.com`. The S3 API endpoint, which is
    /// NOT the public read URL — see `media_base`.
    pub s3_endpoint: String,
    /// Where those objects are *served* from, e.g. `https://images.ourdriveway.com`.
    /// A different hostname from `s3_endpoint`, and the one that goes into events:
    /// this service mints the absolute URL clients store. Must match the base that
    /// spot-service and user-service validate against, or every upload is refused
    /// on the way back in.
    pub media_base: String,
    /// Separate buckets per environment, so local test uploads never land beside
    /// real listings and the dev credential can be scoped away from production.
    pub s3_bucket: String,
    /// `auto` on R2. Signed either way, so it has to match what the store expects.
    pub s3_region: String,
    pub s3_access_key_id: String,
    pub s3_secret_access_key: String,
    /// How long a minted URL stays usable. Long enough to upload a phone photo on
    /// a bad connection, short enough that a leaked URL is not a standing grant.
    pub presign_expiry_secs: u64,
    /// Ceiling for ONE image. Replaces the `DefaultBodyLimit` spot-service used to
    /// carry — see `route::upload_url` for how it is actually enforced.
    pub max_upload_bytes: u64,
}

pub static CONFIG: LazyLock<Config> = LazyLock::new(|| Config {
    port: env::require_parsed("PORT"),
    jwt_secret: env::require("JWT_SECRET"),
    s3_endpoint: env::require("S3_ENDPOINT"),
    media_base: env::require("MEDIA_BASE"),
    s3_bucket: env::require("S3_BUCKET"),
    s3_region: env::require("S3_REGION"),
    s3_access_key_id: env::require("S3_ACCESS_KEY_ID"),
    s3_secret_access_key: env::require("S3_SECRET_ACCESS_KEY"),
    presign_expiry_secs: env::require_parsed("PRESIGN_EXPIRY_SECS"),
    max_upload_bytes: env::require_parsed("MAX_UPLOAD_BYTES"),
});

#[derive(Clone)]
pub struct AppState {
    pub s3: Arc<aws_sdk_s3::Client>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::fmt::init();
    shared::install_default_crypto_provider();

    // Local dev only: in a container the environment comes from the orchestrator
    // and this file does not exist, so the failure is discarded. Read explicitly by
    // path so the variables resolve regardless of the process CWD.
    dotenvy::from_filename("apps/services/media-service/.env").ok();

    // Resolve the whole environment before anything binds a port. Without this a
    // missing variable would surface as a panic inside the first handler that
    // needed it, leaving a process that passes its health check and fails requests.
    LazyLock::force(&CONFIG);
    // Installs the origin `shared::media` mints and validates against. Beside the
    // CONFIG force for the same reason: a missing base must stop the process, not
    // surface as a rejected upload later.
    shared::media::init_base(&CONFIG.media_base);
    shared::init_jwt_decoding_key(&CONFIG.jwt_secret);

    // Credentials come from the Config above rather than a provider chain: this
    // runs in a container with no instance metadata and no ~/.aws, so a chain
    // would only add ways for the wrong credential to be picked up silently.
    let s3 = aws_sdk_s3::Client::from_conf(
        aws_sdk_s3::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new(CONFIG.s3_region.clone()))
            .endpoint_url(&CONFIG.s3_endpoint)
            // R2 and MinIO both accept path style; virtual-hosted would put the
            // bucket into the hostname and make the endpoint per-bucket.
            .force_path_style(true)
            .credentials_provider(Credentials::new(
                &CONFIG.s3_access_key_id,
                &CONFIG.s3_secret_access_key,
                None,
                None,
                "static",
            ))
            .build(),
    );

    let state = AppState { s3: Arc::new(s3) };

    // No `bus::health::routes` here. That readiness means "my projection has
    // caught up with the log", and this service has no projection — it holds no
    // state, consumes no events and talks to no database. Alive IS ready.
    let app = Router::new()
        .route("/api/media/upload-url", post(route::upload_url))
        .route("/healthz", get(ok))
        .route("/readyz", get(ok))
        .with_state(state);

    // PORT differs per service in local dev so several can run on one host.
    // Containerised, every service listens on 80.
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", CONFIG.port)).await?;
    tracing::info!(port = CONFIG.port, bucket = %CONFIG.s3_bucket, "media-service listening");
    Ok(axum::serve(listener, app).await?)
}

async fn ok() -> StatusCode {
    StatusCode::OK
}
