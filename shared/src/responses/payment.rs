use serde::Serialize;
use uuid::Uuid;

/// A checkout the renter can now pay.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionResponse {
    /// The handle the checkout screen navigates with. Everything it needs afterwards —
    /// the client secret, the booking, the outcome — it fetches back from this id, which
    /// is why it is the only thing that ever appears in a checkout URL.
    pub session_id: String,
    /// Saves the checkout screen an immediate round trip on the happy path. Not a secret
    /// in the bearer-token sense: it authorizes paying this one session and nothing else.
    pub client_secret: String,
}

/// What became of a checkout.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionStateResponse {
    /// `complete`, `open` or `expired` — Stripe's own answer, not ours.
    pub status: &'static str,
    /// True when the money has actually arrived, as opposed to a `complete` session whose
    /// asynchronous method is still processing.
    pub paid: bool,
    /// Present while the session is still payable, so the screen can mount the Payment
    /// Element knowing nothing but the id in its URL.
    pub client_secret: Option<String>,
    /// So the screen can release the hold without the booking id ever being in the URL.
    pub booking_id: Uuid,
}

/// A host's money.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EarningsResponse {
    /// Withdrawable now: settled income minus what has already been taken out.
    pub available_cents: i64,
    /// Everything earned and settled, ever.
    pub earned_cents: i64,
    pub paid_out_cents: i64,
}

/// A withdrawal, which like every other write answers with where it landed.
///
/// Carries the amount as well, because the server computed it — the client sent no
/// figure and has no other way to learn what was actually taken out.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PayoutResponse {
    pub seq: String,
    pub amount_cents: i64,
}
