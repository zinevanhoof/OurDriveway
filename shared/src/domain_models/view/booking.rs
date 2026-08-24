use chrono::{DateTime, Utc};
use surrealdb::types::vars;
use surrealdb::types::{Datetime, SurrealValue};
use surrealdb::{engine::remote::ws::Client, method::Query};
use uuid::Uuid;

use crate::{
    domain_models::booking::status,
    events::booking::{BookingCreated, CancelReason, ReleaseReason},
    general_models::booking::Booked,
};

/// The `booking` table in the read model.
///
/// `spot` and `renter` — the two `record<>` links that make nested GraphQL work —
/// are not on this model; see the module doc. A `CONTENT` write built from here
/// therefore clears them, which is why `ViewBookingRepository::created` relinks in
/// the same transaction.
#[derive(Clone, Debug, SurrealValue)]
pub struct ViewBooking {
    pub id: Uuid,
    pub spot_id: Uuid,
    pub owner_id: Uuid,
    pub renter_id: Uuid,
    pub booked: Booked,
    /// EUR cents.
    pub amount: i64,
    pub status: String,
    pub hold_until: Option<DateTime<Utc>>,
    pub release_reason: Option<String>,
    pub cancel_reason: Option<String>,
    /// The renter's score after the trip. NONE until they rate.
    pub rating: Option<i64>,
    pub ends_at: Datetime,
    pub created_at: Datetime,
}

impl ViewBooking {
    /// The row a `Created` writes.
    pub fn created(e: BookingCreated, at: DateTime<Utc>) -> Self {
        Self {
            id: e.booking_id,
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
            ends_at: e.ends_at.into(),
            created_at: at.into(),
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
    /// Binds every patchable column. Absent ones bind as NONE, which the
    /// `?? column` in `ViewBookingRepository::settle` turns into "leave it alone".
    pub fn bind(self, q: Query<'_, Client>) -> Query<'_, Client> {
        q.bind(vars! {
            status:         self.status,
            release_reason: self.release_reason,
            cancel_reason:  self.cancel_reason,
        })
    }

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
