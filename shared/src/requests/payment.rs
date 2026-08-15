use garde::Validate;
use serde::Deserialize;
use uuid::Uuid;

/// What the checkout step posts to start paying.
///
/// One field, and note what is **absent**: an amount. The server takes it from the
/// booking as it priced it at reserve time. A client-supplied figure would be a price
/// the renter chose.
#[derive(Deserialize, Validate)]
#[serde(rename_all = "camelCase")]
pub struct CreateSessionRequest {
    /// serde rejects a malformed uuid before garde runs, so no length rule is
    /// needed — and nothing downstream has to parse it.
    #[garde(skip)]
    pub booking_id: Uuid,

    /// Where Stripe sends the renter after a redirect payment method, including the
    /// literal `{CHECKOUT_SESSION_ID}` placeholder Stripe substitutes.
    ///
    /// The client builds it because only the client knows where it is running: the web
    /// build returns to its own origin, the Tauri build to a `ourdriveway://` deep link.
    /// The server has no way to tell them apart and no business guessing.
    ///
    /// Deliberately only checked for presence. It looks like an open redirect and
    /// effectively isn't: Stripe performs the redirect from *its* own domain, and a caller
    /// supplying its own return URL is redirecting itself after its own payment. There is
    /// no victim to send somewhere.
    ///
    /// Not shape-checked either. `garde`'s `url` rule needs a feature flag that pulls in
    /// the `url` crate, and Stripe already rejects a malformed `return_url` when the
    /// session is created — which surfaces here as a 502 naming the parameter.
    #[garde(length(min = 1))]
    pub return_url: String,
}
