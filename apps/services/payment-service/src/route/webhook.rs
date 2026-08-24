use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use shared::error::myerror::{ContextExt, MyResult};

use crate::{AppState, CONFIG, client::stripe};

/// Stripe's signature header. Not a constant in the crate, so it is one here.
const SIGNATURE: &str = "stripe-signature";

/// The only thing that can confirm a booking.
///
/// Deliberately **no `AuthedJwt`**: Stripe holds no token of ours. Authentication is
/// the signature over the raw body, which is why this takes `Bytes` and not `Json` —
/// deserialising and re-serialising would change the bytes the signature covers and
/// verification could never succeed.
///
/// `Bytes` must be the last extractor; it consumes the body.
pub async fn stripe_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> MyResult<impl IntoResponse> {
    let signature = headers
        .get(SIGNATURE)
        .and_then(|v| v.to_str().ok())
        .context_unauthorized(("Unauthorized", "Missing signature."))?;

    let payload =
        std::str::from_utf8(&body).context_unauthorized(("Unauthorized", "Malformed body."))?;

    // Verifies the HMAC and the timestamp tolerance. An Err here means this did not
    // come from Stripe, so it gets 401 — never 200, which would tell an attacker
    // probing the endpoint that their forgery was accepted.
    let outcome = stripe::verify(payload, signature, &CONFIG.stripe_webhook_secret)?;

    state.payment_service.record_webhook(outcome).await?;

    // 200 even for an event we ignored. A shared sandbox delivers other projects'
    // payments and event types we don't handle; answering anything else makes Stripe
    // retry them with backoff for hours.
    Ok(StatusCode::OK)
}
