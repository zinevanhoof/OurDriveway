use std::sync::LazyLock;

pub mod claims;
pub mod domain_models;
pub mod error;
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
