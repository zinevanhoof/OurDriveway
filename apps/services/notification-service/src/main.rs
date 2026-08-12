use std::sync::{Arc, LazyLock};

use axum::{Router, http::StatusCode, routing::get};
use shared::env;

use crate::{mailer::Mailer, worker::users::UserWorker};

mod mailer;
mod template;
mod worker;

/// Everything this service reads, in one place.
///
/// One-to-one with `apps/services/notification-service/.env`: if a variable is
/// not a field here it is not read, and if it is a field here it is required.
/// Nothing falls back to a default, because a default is a value you cannot
/// discover by reading the `.env`.
pub struct Config {
    pub port: u16,
    pub nats_url: String,
    pub resend_api_key: String,
    /// `Name <address@domain>` or a bare address. The domain must be the one
    /// verified with Resend — mail from anything else is refused outright.
    pub mail_from: String,
    /// Origin the verification link points at, with no trailing slash. In dev
    /// this must be the LAN IP: the link is opened on a phone, where `localhost`
    /// is the phone.
    pub app_base_url: String,
    /// Fills the template's `company_name`. Configured so a dev deployment can
    /// say so and its mail doesn't read as production.
    pub company_name: String,
    /// Signs verification links. **Not** `JWT_SECRET` — see
    /// `shared::email_token`, where the reason is a test.
    pub email_token_secret: String,
    pub verify_token_ttl_secs: i64,
    /// Resend template id or alias.
    pub template_email_verification: String,
}

pub static CONFIG: LazyLock<Config> = LazyLock::new(|| Config {
    port: env::require_parsed("PORT"),
    nats_url: env::require("NATS_URL"),
    resend_api_key: env::require("RESEND_API_KEY"),
    mail_from: env::require("MAIL_FROM"),
    app_base_url: env::require("APP_BASE_URL"),
    company_name: env::require("COMPANY_NAME"),
    email_token_secret: env::require("EMAIL_TOKEN_SECRET"),
    verify_token_ttl_secs: env::require_parsed("VERIFY_TOKEN_TTL_SECS"),
    template_email_verification: env::require("TEMPLATE_EMAIL_VERIFICATION"),
});

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::fmt::init();
    shared::install_default_crypto_provider();

    // Local dev only: in a container the environment comes from the orchestrator
    // and this file does not exist, so the failure is discarded. Read explicitly by
    // path so the variables resolve regardless of the process CWD.
    dotenvy::from_filename("apps/services/notification-service/.env").ok();

    // Resolve the whole environment before anything binds a port. Without this a
    // missing variable would surface as a panic inside the first handler that
    // needed it, leaving a process that passes its health check and fails requests.
    LazyLock::force(&CONFIG);

    // No `init_jwt_decoding_key` and no `JWT_SECRET`: this service authenticates
    // nobody. It has no API surface beyond health, and the only token it touches
    // is the one it mints itself under `EMAIL_TOKEN_SECRET`.

    let js = bus::connect(&CONFIG.nats_url).await?;
    bus::ensure_streams(&js).await?;

    let mailer = Arc::new(Mailer::new(&CONFIG.resend_api_key));

    // `bus::worker`, never `bus::projector`. The projector creates an ephemeral
    // consumer per instance and replays from sequence 1 — which for a side effect
    // means every replica sends its own copy and a fresh pod mails every user who
    // ever registered. The worker's durable consumer is shared across replicas
    // and starts at `New`.
    tokio::spawn(bus::worker::run(
        js.clone(),
        Arc::new(UserWorker {
            mailer: mailer.clone(),
        }),
    ));

    // No `bus::health::routes`. That readiness means "my projection has caught up
    // with the log", and this service has no projection — same as media-service.
    // Alive IS ready.
    let app = Router::new()
        .route("/healthz", get(ok))
        .route("/readyz", get(ok));

    // PORT differs per service in local dev so several can run on one host.
    // Containerised, every service listens on 80.
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", CONFIG.port)).await?;
    tracing::info!(port = CONFIG.port, from = %CONFIG.mail_from, "notification-service listening");
    Ok(axum::serve(listener, app).await?)
}

async fn ok() -> StatusCode {
    StatusCode::OK
}
