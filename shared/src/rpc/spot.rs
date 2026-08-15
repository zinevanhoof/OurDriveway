//! Asking spot-service what a spot looks like.

use serde::{Deserialize, Serialize};
use surrealdb::types::SurrealValue;

/// Subject spot-service answers on. Queue-subscribed, so replicas share the load.
pub const SUBJECT_SPOT_CARD: &str = "spots.card";

/// A spot as something else needs to *display* it. The request is the bare `Uuid`.
///
/// Deliberately not the spot: no price, no availability, no owner, no location. Those are
/// spot-service's to reason about, and a consumer that wants them wants to be a consumer.
/// This is the label on someone else's screen and nothing more, which is what keeps it
/// safe to answer with `None` when anything at all goes wrong.
/// `SurrealValue` so spot-service can select straight into it, as `TimeSlot` and the rest
/// of `shared` already do. Serde is what carries it over NATS.
#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
pub struct SpotCard {
    pub title: String,
    /// `address.formatted` — one line, already assembled for humans.
    pub address: String,
    /// Absolute URLs, straight out of the projection — see [`crate::media`], where
    /// they are stored that way. Nothing is resolved here; this field used to be the
    /// one place a server joined a hostname onto a key, and that merge is gone.
    ///
    /// Absolute is not cosmetic. These go onto a Stripe Checkout Session, and
    /// **Stripe fetches them server-side and re-hosts the image on its own CDN** —
    /// `getSession()` returns a CloudFront URL, never the one sent. A bare key gives
    /// Stripe nothing to fetch: it is stored without complaint and handed back
    /// verbatim, so no photo ever reaches the browser.
    ///
    /// Two things follow. `MEDIA_BASE` must be reachable **from Stripe**, not merely
    /// from a phone — a LAN address or a private bucket fails silently, with no error
    /// on any request we make. And the image a session shows is frozen at creation,
    /// because it is Stripe's copy; editing the spot's photos afterwards does not
    /// change it.
    pub images: Vec<String>,
}
