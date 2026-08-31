use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::{
    domain_models::booking::status,
    events::booking::{BookingCreated, CancelReason, ReleaseReason},
    general_models::booking::Booked,
};

/// The `booking` table in the read model.
///
/// The `spot` and `renter` record links are gone, and with them the whole
/// clear-then-relink dance: a `CONTENT` write built from this model used to erase
/// both, so `ViewBookingRepository::created` had to put them back in the same
/// transaction — two halves that both had to run, with nothing in Rust connecting
/// them. `spot_id` and `renter_id` are plain uuids and a read LEFT JOINs.
#[derive(Clone, Debug, sqlx::FromRow)]
pub struct ViewBooking {
    pub id: Uuid,
    /// booking-service's version of this booking, as last applied here. What
    /// `bus::await_version` compares a client's `X-Await-Version` against.
    #[sqlx(try_from = "i64")]
    pub version: u64,
    pub spot_id: Uuid,
    pub owner_id: Uuid,
    pub renter_id: Uuid,
    #[sqlx(json)]
    pub booked: Booked,
    /// EUR cents.
    pub amount: i64,
    pub status: String,
    pub hold_until: Option<DateTime<Utc>>,
    pub release_reason: Option<String>,
    pub cancel_reason: Option<String>,
    /// The renter's score after the trip. NULL until they rate.
    pub rating: Option<i32>,
    pub ends_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

impl ViewBooking {
    /// The row a `Created` writes.
    pub fn created(e: BookingCreated, at: DateTime<Utc>, version: u64) -> Self {
        Self {
            id: e.booking_id,
            version,
            spot_id: e.spot_id,
            owner_id: e.owner_id,
            renter_id: e.renter_id,
            booked: e.booked,
            amount: e.amount_cents,
            status: status::RESERVED.to_string(),
            hold_until: Some(e.expires_at),
            release_reason: None,
            cancel_reason: None,
            rating: None,
            ends_at: e.ends_at,
            created_at: at,
        }
    }
}

/// A partial update to a [`ViewBooking`], written with the struct-update idiom.
///
/// Three columns, which is every column a settlement writes. Notably **not**
/// `hold_until`: it is only ever cleared, never set, and a `?? column` patch cannot
/// express that — `ViewBookingRepository::settle` clears it with its own trailing
/// assignment. The `spot` and `renter` links are absent for the same structural
/// reason they are absent from [`ViewBooking`]: they belong to `link_refs`.
#[derive(Debug, Default)]
pub struct ViewBookingPatch {
    pub status: Option<String>,
    pub release_reason: Option<String>,
    pub cancel_reason: Option<String>,
}

impl ViewBookingPatch {
    // No `bind` — see the note in `domain_models::user::user`. sqlx binds
    // positionally, so the binds live beside the `$n` placeholders in
    // `ViewBookingRepository::settle`.

    /// Paid.
    pub fn confirmed() -> Self {
        Self {
            status: Some(status::CONFIRMED.to_string()),
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

    /// The SQL lives in `ViewBookingRepository` now, so nothing here can assert on
    /// it. What this *can* do is fail the moment the struct grows a field.
    ///
    /// The literal is **exhaustive on purpose** — no `..Default::default()`. Add a
    /// field to [`ViewBookingPatch`] and this stops compiling, which is the reminder
    /// that `bind` and the `SET` list in `settle` need it too.
    ///
    /// `spot`, `renter` and `hold_until` are absent by construction: the first two
    /// belong to `link_refs`, and the third is cleared by `settle`'s own trailing
    /// assignment because `?? column` can only leave a value alone.
    #[test]
    fn set_covers_every_patchable_column() {
        let _: ViewBookingPatch = ViewBookingPatch {
            status: None,
            release_reason: None,
            cancel_reason: None,
        };
    }

    /// Only one reason is ever written, so a renter's history can tell "your hold
    /// ran out" from "the host pulled the listing" — the status alone cannot.
    #[test]
    fn a_settled_booking_carries_exactly_one_reason() {
        let released = ViewBookingPatch::released(ReleaseReason::Expired);
        assert_eq!(released.release_reason.as_deref(), Some("expired"));
        assert!(released.cancel_reason.is_none());

        let cancelled = ViewBookingPatch::cancelled(CancelReason::ByRenter);
        assert_eq!(cancelled.cancel_reason.as_deref(), Some("by_renter"));
        assert!(cancelled.release_reason.is_none());

        let confirmed = ViewBookingPatch::confirmed();
        assert!(confirmed.release_reason.is_none());
        assert!(confirmed.cancel_reason.is_none());
    }
}
