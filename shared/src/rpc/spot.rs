//! Asking spot-service what a spot looks like.

use serde::{Deserialize, Serialize};

/// Subject spot-service answers on. Queue-subscribed, so replicas share the load.
///
/// `rpc.` and not `spots.`, which is where it used to live. A JetStream stream
/// captures **every** message on a subject it binds, request/reply included, so
/// `spots.card` matched the SPOTS stream's `spots.>` and every RPC request was
/// stored as if it were an event. The projectors on that stream have no
/// `filter_subject` narrow enough to have excluded it before partitioning, so one
/// `SpotCard` lookup was enough to feed a bare `Uuid` to
/// `serde_json::from_slice::<Envelope<SpotEvent>>` and stop both SPOTS projectors
/// for good.
///
/// Nothing binds `rpc.>`, which is the point. `only_event_subjects_land_in_streams`
/// in `crate::events` is what keeps it that way.
pub const SUBJECT_SPOT_CARD: &str = "rpc.spot.card";

/// A spot as something else needs to *display* it. The request is the bare `Uuid`.
///
/// Deliberately not the spot: no price, no availability, no host, no location. Those are
/// spot-service's to reason about, and a consumer that wants them wants to be a consumer.
/// This is the label on someone else's screen and nothing more, which is what keeps it
/// safe to answer with `None` when anything at all goes wrong.
/// Serde is what carries it over NATS, and is now the only derive it needs. It used to
/// also be `SurrealValue` so spot-service could select straight into it; the card is
/// built from an already-read `Spot` through the `From` impl in `domain_models::spot`,
/// so there is no second query shape to keep in step with this one.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
