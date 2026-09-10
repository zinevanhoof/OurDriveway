use chrono::{DateTime, Utc};
use diesel::prelude::*;
use uuid::Uuid;

use crate::{
    domain_models::booking::status,
    events::booking::{BookingCreated, CancelReason, ReleaseReason},
    general_models::booking::Booked,
};

/// payment-service's `booking` table — a **mirror**, not the booking.
///
/// A different table in a different database from
/// [`crate::domain_models::booking::Booking`], holding what this service needs to
/// authorize a payment and decide a refund: what it costs, whose it is, and whether
/// it is still going to happen. Note the column is `amount_cents` here and `amount`
/// there — two tables, not one.
///
/// Unlike booking-service's spot mirror, **nothing here is `Option` except the
/// genuinely optional columns**. `BookingCreated` is always the first event for a
/// booking and BOOKINGS never expires (`max_age` None), so a replay from sequence 1
/// can only ever create this row complete.
#[derive(Clone, Debug, Queryable, Selectable, Insertable, AsChangeset)]
#[diesel(table_name = crate::schema::payment::booking)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct BookingMirror {
    pub id: Uuid,
    /// Which spot, so checkout can ask spot-service what to call it. The id alone —
    /// this service holds no spot data and consumes no SPOTS events.
    pub spot_id: Uuid,
    pub host_id: Uuid,
    pub renter_id: Uuid,
    pub amount_cents: i64,
    /// Wall-clock slots in the spot's zone, projected for exactly one reason: the
    /// Checkout Session's line item, which is the only description of the purchase
    /// that ever reaches the renter's payment screen or their Stripe receipt.
    ///
    /// Rendered literally, so this service still needs no timezone.
    ///
    /// A `jsonb` column, `NOT NULL DEFAULT '{}'` — so the `booked ?? {}` that used to
    /// be in every statement selecting this table is gone with the absent case.
    pub booked: Booked,
    /// One of `domain_models::booking::status`. `completed` is deliberately absent
    /// from this table's CHECK — nothing publishes it, and "the host has earned
    /// this" is `confirmed` plus an `ends_at` past the settlement window.
    pub status: String,
    /// When the hold lapses; NONE once the booking is no longer reserved. Read when
    /// creating a payment: an expired hold must not be payable.
    pub hold_until: Option<DateTime<Utc>>,
    /// The last moment the booking occupies. The field the settlement window is
    /// measured against, projected rather than recomputed from `booked` because that
    /// map is wall-clock strings that cannot be compared to `now` without the zone.
    pub ends_at: DateTime<Utc>,
    /// Why it was withdrawn, when it was. Carried so a refund policy could one day
    /// distinguish a host cancelling from a renter cancelling; today every refund is
    /// the full amount and these are written but unread.
    pub cancel_reason: Option<String>,
    pub release_reason: Option<String>,
}

impl BookingMirror {
    /// The row a `BookingCreated` mirrors.
    pub fn created(e: BookingCreated) -> Self {
        Self {
            id: e.booking_id,
            spot_id: e.spot_id,
            host_id: e.host_id,
            renter_id: e.renter_id,
            amount_cents: e.amount_cents,
            booked: e.booked,
            status: status::RESERVED.to_string(),
            hold_until: Some(e.expires_at),
            ends_at: e.ends_at,
            cancel_reason: None,
            release_reason: None,
        }
    }
}

/// A partial update to a [`BookingMirror`], written with the struct-update idiom:
///
/// ```ignore
/// BookingMirrorPatch { status: Some(…), ..Default::default() }
/// ```
///
/// Three columns, which is every column any transition writes. Notably **not**
/// `hold_until`: it is only ever cleared, never set, and a `?? column` patch cannot
/// express that — see [`BookingMirrorPatch::status`] and
/// `BookingMirrorRepository::transition`.
#[derive(Debug, Default, AsChangeset)]
#[diesel(table_name = crate::schema::payment::booking)]
pub struct BookingMirrorPatch {
    pub status: Option<String>,
    pub cancel_reason: Option<String>,
    pub release_reason: Option<String>,
}

impl BookingMirrorPatch {
    // No `bind` — `AsChangeset` is the `SET` list. See the note in
    // `domain_models::user::user`. What `AsChangeset` will NOT write is a column back
    // to NULL, so `transition` assigns `hold_until` beside the patch; the live
    // round-trip in payment-service is what catches that going missing.

    /// Paid. Clears the hold, which no longer means anything.
    ///
    /// `hold_until` cannot be cleared through a patch — `COALESCE($n, column)` can
    /// only leave a value alone — so the projector's conditional update sets it to
    /// NULL alongside. See `BookingMirrorRepository::transition`.
    pub fn status(to: &str) -> Self {
        Self {
            status: Some(to.to_string()),
            ..Self::default()
        }
    }

    /// The hold ended without being paid.
    pub fn released(reason: ReleaseReason) -> Self {
        Self {
            status: Some(status::RELEASED.to_string()),
            release_reason: Some(reason.as_str().to_string()),
            ..Self::default()
        }
    }

    /// A paid booking was withdrawn.
    pub fn cancelled(reason: CancelReason) -> Self {
        Self {
            status: Some(status::CANCELLED.to_string()),
            cancel_reason: Some(reason.as_str().to_string()),
            ..Self::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The SQL lives in `BookingMirrorRepository` now, so nothing here can assert
    /// on it. What this *can* do is fail the moment the struct grows a field.
    ///
    /// The literal is **exhaustive on purpose** — no `..Default::default()`. Add a
    /// field to [`BookingMirrorPatch`] and this stops compiling, which is the
    /// reminder that `bind` and the `SET` list in `transition` need it too.
    ///
    /// `hold_until` is deliberately absent: it is cleared by that statement's
    /// trailing `hold_until = NONE`, never patched, and adding it here would put the
    /// two assignments in a fight.
    #[test]
    fn set_covers_every_patchable_column() {
        let _: BookingMirrorPatch = BookingMirrorPatch {
            status: None,
            cancel_reason: None,
            release_reason: None,
        };
    }

    /// The two reasons are separate columns and only ever one is written. Collapsing
    /// them would leave a renter's history unable to tell "your hold ran out" from
    /// "the host pulled the listing".
    #[test]
    fn a_settled_booking_carries_exactly_one_reason() {
        let released = BookingMirrorPatch::released(ReleaseReason::Expired);
        assert_eq!(released.release_reason.as_deref(), Some("expired"));
        assert!(released.cancel_reason.is_none());

        let cancelled = BookingMirrorPatch::cancelled(CancelReason::SpotUnavailable);
        assert_eq!(cancelled.cancel_reason.as_deref(), Some("spot_unavailable"));
        assert!(cancelled.release_reason.is_none());
    }
}
