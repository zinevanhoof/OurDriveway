use chrono::{DateTime, Utc};
use diesel::prelude::*;
use uuid::Uuid;

use crate::{events::booking::BookingCreated, general_models::booking::Booked};

/// A booking's lifecycle, as stored in `status`.
///
/// Bare `&str` rather than an enum because that is what the column is and what
/// every guard compares against; the schema's `CHECK (status IN (…))` is the
/// authority. Here so the strings are written once — a typo in one of them is a
/// transition that silently never matches.
pub mod status {
    /// A hold taken while checkout runs. Blocks slots exactly like a confirmed
    /// booking, and ends as `CONFIRMED` or `RELEASED`.
    pub const RESERVED: &str = "reserved";
    /// Paid.
    pub const CONFIRMED: &str = "confirmed";
    /// The renter backed out, or the hold lapsed and the sweeper collected it.
    pub const RELEASED: &str = "released";
    /// A *paid* booking withdrawn — by the renter, or because the host pulled the
    /// spot out from under it.
    pub const CANCELLED: &str = "cancelled";
}

/// The `booking` table, whole.
///
/// Read entire rather than per-use-case: this replaced a `BookingForUpdate` that
/// selected seven columns for the write path and a `LiveBooking` that selected
/// three for the cancel reactor, both over a row addressed by primary key.
#[derive(Clone, Debug, Queryable, Selectable, Insertable, AsChangeset)]
#[diesel(table_name = crate::schema::booking::booking)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct Booking {
    pub id: Uuid,
    /// Bumped by booking-service inside the transaction that writes this row.
    /// The version a client waits on, and the gap detector for an out-of-order event —
    /// see `shared::events::Envelope`. No longer the key concurrent writers collide
    /// on; see `shared::db::next_version`.
    pub version: i64,
    /// A plain uuid, not a record link: the `spot` table lives in spot-service's
    /// database, so this one cannot hold a `record<spot>`.
    pub spot_id: Uuid,
    /// Denormalised at create time so a booking can be scoped to the host without
    /// a cross-database dereference.
    pub host_id: Uuid,
    pub renter_id: Uuid,
    /// `"YYYY-MM-DD"` -> slots, in the spot's timezone. Bare wall-clock strings,
    /// which is why `ends_at` exists as a separate folded instant.
    ///
    /// A `jsonb` column. It was a SCHEMAFULL object spelled out to
    /// `booked.*.*.start`, which bought nothing: no query has ever reached into it,
    /// and the shape is enforced by garde on the request that creates it.
    pub booked: Booked,
    /// EUR cents, recomputed server-side from the minutes actually authorised —
    /// never a figure the client sent.
    pub amount: i64,
    /// One of [`status`]. A string because the schema's CHECK constrains the set.
    pub status: String,
    /// When a hold lapses. **The only place a hold expiry is stored**, and
    /// deliberately not part of what makes this booking block a slot: a `reserved`
    /// row blocks, full stop. The sweeper publishing `Released` is what frees it, so
    /// no reader has to compare this against a clock.
    pub hold_until: Option<DateTime<Utc>>,
    pub release_reason: Option<String>,
    pub cancel_reason: Option<String>,
    /// The last moment this booking occupies, as an instant. Folded on the write
    /// side because `booked` is wall-clock and answering "is it over" from it needs
    /// the spot's zone, which a query does not have.
    pub ends_at: DateTime<Utc>,
    /// The renter's score after the trip. NULL until they rate.
    pub rating: Option<i32>,
    pub created_at: DateTime<Utc>,
}

// There is deliberately no `BookingPatch`. Every write to this table is either a
// whole-row `upsert` or `BookingRepository::transition`, which binds its columns
// directly — a transition's value is its `WHERE`, and a patch struct cannot carry
// one.

impl Booking {
    /// The row a `Created` writes.
    ///
    /// Lands in [`status::RESERVED`], which is deliberately *not* renamed with the
    /// event: creating the booking is the action, being a hold is the state it
    /// starts in, and the row goes on to be `confirmed` or `released` from there.
    ///
    /// `at` is the envelope's clock, never this process's — every replica has to
    /// store the same `created_at` for the same event.
    pub fn created(e: BookingCreated, at: DateTime<Utc>, version: i64) -> Self {
        Self {
            id: e.booking_id,
            version,
            spot_id: e.spot_id,
            host_id: e.host_id,
            renter_id: e.renter_id,
            booked: e.booked,
            amount: e.amount_cents,
            status: status::RESERVED.to_string(),
            hold_until: Some(e.expires_at),
            release_reason: None,
            cancel_reason: None,
            ends_at: e.ends_at.into(),
            rating: None,
            created_at: at.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh hold is `reserved` with its expiry set. Both halves matter: the
    /// projector's transitions are all scoped `WHERE status IN [...]`, and the
    /// hold sweeper selects on `hold_until` — a row created without one is a hold
    /// that never lapses.
    #[test]
    fn a_reserved_booking_starts_held() {
        let e = BookingCreated {
            booking_id: Uuid::now_v7(),
            spot_id: Uuid::now_v7(),
            host_id: Uuid::now_v7(),
            renter_id: Uuid::now_v7(),
            booked: Default::default(),
            amount_cents: 500,
            expires_at: "2026-08-03T12:00:00Z".parse().unwrap(),
            ends_at: "2026-08-03T18:00:00Z".parse().unwrap(),
        };
        let row = Booking::created(e, Utc::now(), 1);
        assert_eq!(row.status, status::RESERVED);
        assert!(row.hold_until.is_some());
    }
}
