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
    /// A booking exists. Its slots are held from this moment — they block other
    /// renters immediately, and keep blocking until this is confirmed, released, or
    /// the hold is swept for expiry.
    ///
    /// Named for the booking, not the hold: holding the slots is what creating one
    /// entails. The row's `status` is still `reserved`, because that is the state
    /// it lands in, and the two words are not interchangeable — see
    /// `domain_models::booking::status`.
    Created(BookingCreated),
    /// Payment succeeded. The hold becomes permanent.
    Confirmed { booking_id: Uuid },
    /// The hold is given up and the slots go back on the market.
    Released {
        booking_id: Uuid,
        reason: ReleaseReason,
    },
    /// A paid booking is withdrawn — by the renter up to an hour before it starts,
    /// or by the system when the host makes the spot unable to honour it.
    ///
    /// Its own variant rather than a third `ReleaseReason`, because what blocks a
    /// slot is keyed off the row's *status*: a `Released { reason: Cancelled }`
    /// would still have to land as `status = 'cancelled'`, so both projectors would
    /// need to branch on the reason to pick the target status *and* the allowed
    /// `from` —
    /// two branches in two files instead of one straight arm each. It also keeps
    /// "your hold ran out" and "you cancelled a booking you paid for" apart in the
    /// one field a renter's history already reads.
    ///
    /// This is the *only* event that withdraws money already taken, which makes it
    /// the single trigger a payment service has to subscribe to for refunds.
    Cancelled {
        booking_id: Uuid,
        reason: CancelReason,
    },
}

/// Who withdrew a paid booking. The refund rules differ — a host who cancels owes
/// the full amount back, a renter cancelling inside the window may not — so the
/// payment service can't be left to guess from an unqualified `cancelled`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CancelReason {
    /// The renter withdrew, in time.
    ByRenter,
    /// The host deleted the listing or narrowed its hours out from under a booking.
    SpotUnavailable,
}

impl CancelReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ByRenter => "by_renter",
            Self::SpotUnavailable => "spot_unavailable",
        }
    }

    /// The inverse, for reading the column back out of a row — which is what a
    /// backfill does when it re-emits a cancellation from current state.
    ///
    /// Beside [`Self::as_str`] so the two cannot drift, and total rather than
    /// `Option`: the schema asserts the set, so an unrecognised string is a
    /// corrupted row, and defaulting a *renter's* cancellation onto the host is the
    /// safer way to be wrong — it is the reason that owes a full refund.
    pub fn parse(s: &str) -> Self {
        match s {
            "by_renter" => Self::ByRenter,
            _ => Self::SpotUnavailable,
        }
    }
}

/// Why a hold ended without becoming a booking. Costs nothing to carry and it's
/// the difference between "you cancelled" and "your hold ran out" in a renter's
/// history — indistinguishable once the row is just `released`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReleaseReason {
    /// The renter backed out of checkout.
    Abandoned,
    /// The hold lapsed and the hold sweeper collected it.
    Expired,
}

impl ReleaseReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Abandoned => "abandoned",
            Self::Expired => "expired",
        }
    }

    /// The inverse, for the same reason as [`CancelReason::parse`]. Neither answer
    /// costs anyone money here — this is the difference between "you backed out"
    /// and "your hold ran out" in a renter's history.
    pub fn parse(s: &str) -> Self {
        match s {
            "abandoned" => Self::Abandoned,
            _ => Self::Expired,
        }
    }
}

impl BookingEvent {
    /// The booking every variant is about — the aggregate half of `booking:<uuid>`.
    pub fn booking_id(&self) -> Uuid {
        match self {
            Self::Created(e) => e.booking_id,
            Self::Confirmed { booking_id }
            | Self::Released { booking_id, .. }
            | Self::Cancelled { booking_id, .. } => *booking_id,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BookingCreated {
    pub booking_id: Uuid,
    /// Selects the subject this booking lives on: every booking for one spot
    /// shares that spot's subject, which is what gives them a total order.
    pub spot_id: Uuid,
    /// Denormalized from the spot so the booking row can be scoped to the owner
    /// without a cross-database dereference.
    pub owner_id: Uuid,
    /// From the verified JWT claim.
    pub renter_id: Uuid,
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
    /// The last moment this booking occupies, as a UTC instant.
    ///
    /// `booked` alone can't answer "is this still to come" in a query: it is a map
    /// of wall-clock strings in the spot's zone, so every reader would have to fold
    /// it *and* know the zone. Folded once here instead, which is what lets both the
    /// host's and the renter's list filter on a single indexed field.
    pub ends_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `as_str` writes the column, `parse` reads it back. They are two matches over
    /// the same strings, so nothing but this stops one gaining a variant the other
    /// does not — and the failure would be quiet: a backfilled cancellation
    /// attributed to the wrong party, which is the difference between a full refund
    /// and none.
    #[test]
    fn every_reason_round_trips_through_its_column() {
        for reason in [CancelReason::ByRenter, CancelReason::SpotUnavailable] {
            assert_eq!(CancelReason::parse(reason.as_str()), reason);
        }
        for reason in [ReleaseReason::Abandoned, ReleaseReason::Expired] {
            assert_eq!(ReleaseReason::parse(reason.as_str()), reason);
        }
    }
}
