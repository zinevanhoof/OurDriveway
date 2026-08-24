use std::sync::OnceLock;

use jsonwebtoken::DecodingKey;

// Lets `::shared::…` paths resolve *inside* this crate too, where the models
// themselves live. Same trick serde uses. It was here for `#[derive(Row)]`, which
// emitted absolute paths; that macro is gone, but doc links and any future derive
// still want this.
extern crate self as shared;

pub mod claims;
pub mod db;
pub mod domain_models;
pub mod email_token;
pub mod env;
pub mod error;
pub mod events;
pub mod extract;
pub mod extractors;
pub mod general_models;
pub mod media;
pub mod notification;
pub mod requests;
pub mod responses;
pub mod rpc;
pub(crate) mod validation;

/// rustls ends up compiled with BOTH the `aws-lc-rs` and `ring` providers (pulled
/// in through surrealdb/hyper-rustls across the workspace), so it can't auto-pick
/// one and panics on first use. Pick aws-lc-rs explicitly. Call once at startup.
pub fn install_default_crypto_provider() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
}

/// The key `AuthedJwt` verifies with.
///
/// This crate used to own a `SHARED_CONFIG` that read `JWT_SECRET` from the
/// environment itself, which meant part of every service's environment was
/// declared in a library it merely depended on — invisible from the service's own
/// `.env`. Now each service declares `JWT_SECRET` in its own `Config` and hands
/// the value here, so the `.env` and the `Config` next to it are the whole list.
static JWT_DECODING_KEY: OnceLock<DecodingKey> = OnceLock::new();

/// Installs the verification key. Call once from `main`, right after forcing that
/// service's `CONFIG`.
///
/// A repeat call is ignored rather than fatal: the value comes from a `LazyLock`
/// that resolves once, so a second call can only be carrying the same secret.
pub fn init_jwt_decoding_key(secret: &str) {
    let _ = JWT_DECODING_KEY.set(DecodingKey::from_secret(secret.as_bytes()));
}

/// Panics if `main` never installed the key. Unreachable in practice — the call
/// sits beside `LazyLock::force` in all four services — and failing loudly at the
/// first authenticated request still beats rejecting every one of them.
pub(crate) fn jwt_decoding_key() -> &'static DecodingKey {
    JWT_DECODING_KEY
        .get()
        .expect("init_jwt_decoding_key must be called from main before serving requests")
}
