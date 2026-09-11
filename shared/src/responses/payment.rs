use serde::Serialize;
use uuid::Uuid;

/// What `POST /api/payment/session` answers a
/// [`crate::requests::payment::CreateSessionRequest`] with: a checkout the renter can now
/// pay.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSessionResponse {
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

// `EarningsResponse` was here. A host's money is `projections::wallet::Balance` now,
// served by view-service — same three figures plus what is still pending, which needs
// the read model's booking rows to compute.

/// A withdrawal, which like every other write answers with where it landed.
///
/// Carries the amount as well, and that is not a convenience: the client asks for a
/// figure but the server recomputes under the lock and may pay **less** — a booking
/// that had not settled when the page was drawn is gone from the balance by the time
/// the request arrives. This is the only figure that is true, which is why the screen
/// prints this one back rather than the one it sent.
///
/// The version this write reached leaves as the `X-Version` header rather than a
/// field here — see [`super::common::X_VERSION`].
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PayoutResponse {
    pub amount_cents: i64,
}

/// Whether this host can be paid, for the withdraw screen's one gate.
///
/// Answered from Stripe live rather than from a column — see the note over
/// `connect_account` in `migrations/payment/0003`. A cached `enabled` that has gone
/// stale is exactly the failure worth avoiding: it shows a host a withdraw form Stripe
/// then refuses.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectStatusResponse {
    /// `needs_country` — no account, and no country on the profile to open one with.
    /// Accounts v2 fixes `identity.country` permanently at creation, so it is asked for
    /// before anything is created rather than defaulted.
    /// `none` — ready to onboard, nothing created at Stripe yet.
    /// `onboarding` — an account exists but Stripe will not pay it yet.
    /// `enabled` — payouts are on; the withdraw form is safe to show.
    pub state: &'static str,
    /// The last four of the bank account Stripe will pay into, for the summary line.
    /// `None` whenever Stripe does not hand one back, which the screen renders by
    /// omitting the line rather than by inventing a placeholder.
    pub bank_last4: Option<String>,
}

/// The short-lived secret that lets the browser mount Connect's embedded components.
///
/// Not a bearer token for our API: it authorises rendering one account's onboarding and
/// account-management UI, from Stripe's own iframes, and expires on its own.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountSessionResponse {
    pub client_secret: String,
}
