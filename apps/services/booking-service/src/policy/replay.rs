//! The events that reproduce a booking's current row.
//!
//! Used only by `BookingService::backfill`, and here rather than there because it is
//! a pure function over one row — which is also what lets the table below run with
//! no database and no NATS.

use shared::{
    domain_models::booking::{Booking, status},
    events::booking::{BookingCreated, BookingEvent, CancelReason, ReleaseReason},
};

/// The chain a booking actually took to reach the state it is in.
///
/// **Not just the final event**, and that is the whole subtlety. Every consumer of
/// BOOKINGS guards its transitions with `WHERE status IN [...]` — that guard is what
/// makes an out-of-order event a silent no-op, and it is equally what makes a lone
/// `Cancelled` land on a `reserved` row and do nothing at all. So a cancelled booking
/// is re-emitted as created → confirmed → cancelled: the path it took, and the only
/// one those guards accept.
///
/// An unknown status yields the create alone. The schema asserts the set, so that
/// means a corrupt row, and leaving it visible as a hold beats dropping it silently.
pub fn events(booking: &Booking) -> Vec<BookingEvent> {
    let booking_id = booking.id;

    let created = BookingCreated {
        booking_id,
        spot_id: booking.spot_id,
        owner_id: booking.owner_id,
        renter_id: booking.renter_id,
        booked: booking.booked.clone(),
        amount_cents: booking.amount,
        // Cleared when a booking settles, so a settled one has none to recover.
        // Harmless: the settlement that follows clears it again, and until then it
        // is a hold expiry in the past.
        expires_at: booking.hold_until.unwrap_or_else(|| booking.created_at.into()),
        ends_at: booking.ends_at.clone().into(),
    };

    let mut events = vec![BookingEvent::Created(created)];

    match booking.status.as_str() {
        // Where a booking starts. Nothing to add.
        status::RESERVED => {}

        status::CONFIRMED => events.push(BookingEvent::Confirmed { booking_id }),

        status::RELEASED => events.push(BookingEvent::Released {
            booking_id,
            reason: ReleaseReason::parse(booking.release_reason.as_deref().unwrap_or_default()),
        }),

        // Two, not one: only a *paid* booking can be cancelled, so the confirmation
        // it passed through has to be replayed for the cancellation's
        // `WHERE status IN ['confirmed']` to match.
        status::CANCELLED => {
            events.push(BookingEvent::Confirmed { booking_id });
            events.push(BookingEvent::Cancelled {
                booking_id,
                reason: CancelReason::parse(booking.cancel_reason.as_deref().unwrap_or_default()),
            });
        }

        other => tracing::warn!(%booking_id, status = other, "unknown booking status"),
    }

    events
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use uuid::Uuid;

    fn booking(status: &str, release_reason: Option<&str>, cancel_reason: Option<&str>) -> Booking {
        Booking {
            id: Uuid::now_v7(),
            version: 3,
            spot_id: Uuid::now_v7(),
            owner_id: Uuid::now_v7(),
            renter_id: Uuid::now_v7(),
            booked: Default::default(),
            amount: 500,
            status: status.to_string(),
            hold_until: None,
            release_reason: release_reason.map(str::to_string),
            cancel_reason: cancel_reason.map(str::to_string),
            ends_at: Utc::now().into(),
            rating: None,
            created_at: Utc::now().into(),
        }
    }

    /// The chain has to *start* with a create, whatever the status. Only `Created`
    /// brings a row into existence downstream — every other variant is an UPDATE
    /// that matches nothing if the row is not there, which is precisely the case a
    /// rebuild is for.
    #[test]
    fn every_chain_begins_by_creating_the_row() {
        for status in [
            status::RESERVED,
            status::CONFIRMED,
            status::RELEASED,
            status::CANCELLED,
            "nonsense-from-a-corrupt-row",
        ] {
            let events = events(&booking(status, None, None));
            assert!(
                matches!(events.first(), Some(BookingEvent::Created(_))),
                "{status} did not start with Created"
            );
        }
    }

    /// The one that is easy to get wrong, and silently. A cancelled booking needs
    /// the confirmation replayed in between, or the `Cancelled` lands on a
    /// `reserved` row, matches no guard, and the rebuild quietly shows the booking
    /// as a live hold.
    #[test]
    fn a_cancellation_replays_the_confirmation_it_passed_through() {
        let row = booking(status::CANCELLED, None, Some("by_renter"));
        let events = events(&row);

        assert!(matches!(
            events.as_slice(),
            [
                BookingEvent::Created(_),
                BookingEvent::Confirmed { .. },
                BookingEvent::Cancelled {
                    reason: CancelReason::ByRenter,
                    ..
                }
            ]
        ));
    }

    #[test]
    fn each_terminal_status_ends_on_its_own_event() {
        assert!(matches!(
            events(&booking(status::CONFIRMED, None, None)).last(),
            Some(BookingEvent::Confirmed { .. })
        ));
        assert!(matches!(
            events(&booking(status::RELEASED, Some("abandoned"), None)).last(),
            Some(BookingEvent::Released {
                reason: ReleaseReason::Abandoned,
                ..
            })
        ));
        // A hold is already in the state `Created` lands in.
        assert_eq!(events(&booking(status::RESERVED, None, None)).len(), 1);
    }

    /// A hold that has settled has no `hold_until` left to re-emit. It must still
    /// produce a create — falling back to the row's own creation time — rather than
    /// panicking on the missing column.
    #[test]
    fn a_settled_booking_has_no_hold_left_to_recover() {
        let row = booking(status::CONFIRMED, None, None);
        let Some(BookingEvent::Created(created)) = events(&row).into_iter().next() else {
            panic!("expected a create");
        };
        assert_eq!(created.expires_at, row.created_at.into());
    }
}
