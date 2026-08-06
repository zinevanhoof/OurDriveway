use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::general_models::spot::TimeSlot;

/// Everything that can happen to a booking. Published by booking-service only.
///
/// Every booking for one spot lands on a single subject (`booking_subject`), which
/// is what makes a per-spot compare-and-swap possible — see `bus::publish_expecting`.
/// That per-spot total order is the only thing preventing a double booking; nothing
/// in any database enforces it.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum BookingEvent {
    /// Slots are held. They block other renters immediately, and keep blocking
    /// until this is confirmed, released, or the hold is swept for expiry.
    Reserved(BookingReserved),
    /// Payment succeeded. The hold becomes permanent.
    Confirmed { booking_id: Uuid },
    /// The hold is given up and the slots go back on the market.
    Released {
        booking_id: Uuid,
        reason: ReleaseReason,
    },
    /// A paid booking is withdrawn by the renter, up to an hour before it starts.
    ///
    /// Its own variant rather than a third `ReleaseReason`, because `fold_booked`
    /// keys off the row's *status*: a `Released { reason: Cancelled }` would still
    /// have to land as `status = 'cancelled'`, so both projectors would need to
    /// branch on the reason to pick the target status *and* the allowed `from` —
    /// two branches in two files instead of one straight arm each. It also keeps
    /// "your hold ran out" and "you cancelled a booking you paid for" apart in the
    /// one field a renter's history already reads.
    Cancelled { booking_id: Uuid },
}

/// Why a hold ended without becoming a booking. Costs nothing to carry and it's
/// the difference between "you cancelled" and "your hold ran out" in a renter's
/// history — indistinguishable once the row is just `released`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReleaseReason {
    /// The renter backed out of checkout.
    Abandoned,
    /// The hold lapsed and the expiry sweeper collected it.
    Expired,
}

impl ReleaseReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Abandoned => "abandoned",
            Self::Expired => "expired",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BookingReserved {
    pub booking_id: Uuid,
    pub spot_id: Uuid,
    /// The spot's shard, echoed so downstream never recomputes it. It selects the
    /// subject this booking lives on, and a recomputed value would split a spot's
    /// history across two subjects the moment SHARD_COUNT changed.
    pub spot_shard: String,
    /// `"user:abc"` — denormalized from the spot so the booking row can be scoped
    /// to the owner without a cross-database dereference.
    pub owner_id: String,
    /// `"user:abc"`, from the verified JWT claim.
    pub renter_id: String,
    /// `"YYYY-MM-DD"` -> slots, in the spot's timezone. Same shape as a spot's
    /// single-day availability, minus the weekly recurrence.
    pub booked: HashMap<String, Vec<TimeSlot>>,
    /// EUR cents, computed server-side from the spot's price and the *authorised*
    /// minutes. The client's figure is display-only and never reaches this.
    pub amount_cents: i64,
    /// When the hold lapses. Computed once here, before publishing, because a
    /// projector deriving it from its own clock would give every replica a
    /// different answer.
    pub expires_at: DateTime<Utc>,
}
