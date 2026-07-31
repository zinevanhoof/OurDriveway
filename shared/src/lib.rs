use std::sync::LazyLock;

pub mod claims;
pub mod db;
pub mod domain_models;
pub mod error;
pub mod events;
pub mod extract;
pub mod extractors;
pub mod general_models;
pub mod requests;
pub mod responses;

/// rustls ends up compiled with BOTH the `aws-lc-rs` and `ring` providers (pulled
/// in through surrealdb/hyper-rustls across the workspace), so it can't auto-pick
/// one and panics on first use. Pick aws-lc-rs explicitly. Call once at startup.
pub fn install_default_crypto_provider() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
}

pub struct SharedConfig {
    pub jwt_secret: String,
}

static SHARED_CONFIG: LazyLock<SharedConfig> = LazyLock::new(|| SharedConfig {
    jwt_secret: std::env::var("JWT_SECRET").expect("JWT_SECRET must be set"),
});

/// Forces the shared config to resolve at startup.
///
/// Without this a missing JWT_SECRET stays invisible until the first request
/// that needs it, then panics inside a handler and poisons the LazyLock — so
/// the service passes its health check and fails every authenticated request.
/// Call once from main, right after loading the environment.
pub fn check_config() {
    LazyLock::force(&SHARED_CONFIG);
}
