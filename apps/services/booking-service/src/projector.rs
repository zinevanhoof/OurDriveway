use bus::Projector;
use chrono::{DateTime, Utc};
use shared::{
    domain_models::booking::{Booking, SpotMirrorPatch, status},
    error::myerror::MyResult,
    events::{
        Envelope, STREAM_SPOTS,
        booking::{BookingEvent, CancelReason},
        aggregate_id, booking_subject,
        spot::SpotEvent,
    },
    general_models::{booking::Booked, spot::Availability},
};
use surrealdb::{engine::remote::ws::Client, method::Transaction};
use uuid::Uuid;

use crate::policy::availability;
use crate::repository::{
    booking_repository::BookingRepository, spot_mirror_repository::SpotMirrorRepository,
};

/// booking-service consumes SPOTS as well as its own stream: it needs price,
/// availability and active-ness to authorize and price a booking server-side, and
/// the spot table lives in another service's database.
///
/// The two advance independently, which is exactly why a BOOKINGS event can arrive
/// for a spot this projector hasn't created yet — see the `option<>` fields in
/// booking-schema.surql, and `Repository::merge`, which is what lets either
/// projector create the row.
///
/// Holds nothing at all: `bus::Tx` hands it a `&Transaction` per event, and the
/// cancellations it raises go into `_outbox` on that same transaction rather than
/// to NATS directly.
pub struct SpotProjector;

impl Projector for SpotProjector {
    const STREAM: &'static str = STREAM_SPOTS;
    const DURABLE: &'static str = "booking-spots";
    type Event = SpotEvent;

    async fn apply(
        &self,
        tx: &Transaction<Client>,
        event: SpotEvent,
        at: DateTime<Utc>,
        version: u64,
    ) -> MyResult<()> {
        let bookings = BookingRepository { q: tx };
        let spots = SpotMirrorRepository { q: tx };
        let spot_id = event.spot_id();

        // React before projecting. Both now sit inside one transaction, so on any
        // failure the cursor stays put and the whole thing runs again — the cancels
        // are deduped by their deterministic event ids and the mirror write is an
        // idempotent merge.
        //
        // Nothing is read from the projection that the event doesn't already carry,
        // so running first costs nothing in accuracy.
        Self::react(tx, &bookings, &event, at).await?;

        // `merge`, never `upsert`: CONTENT would erase `booked` and `bookings_seq`,
        // which this stream does not own and the BOOKINGS projector may already
        // have written.
        match event {
            SpotEvent::Created(e) => spots.merge(e.spot_id, SpotMirrorPatch::created(e)).await,
            SpotEvent::Updated(e) => {
                let spot_id = e.spot_id;
                spots.merge(spot_id, SpotMirrorPatch::updated(e)).await
            }
            SpotEvent::Deleted { spot_id } => {
                spots.merge(spot_id, SpotMirrorPatch::deleted()).await
            }
        }?;

        // The SPOTS aggregate version, kept beside `bookings_seq` on the same mirror
        // row. Two counters, two jobs: this one tracks spot-service's writes, that
        // one is this service's per-spot booking cursor.
        shared::db::set_version(tx, "spot", &spot_id, version).await
    }
}

impl SpotProjector {
    /// Withdraws bookings the spot can no longer honour.
    ///
    /// This is where a host's edit meets the bookings it invalidates, and it lives
    /// here — not in spot-service — because *this* stream is the one with a per-spot
    /// total order over bookings. spot-service publishes its edit without ever
    /// knowing a booking exists.
    ///
    /// Only *confirmed* bookings are touched. A live hold lapses within `HOLD` on its
    /// own, so releasing it here would buy fifteen minutes at the cost of racing a
    /// payment — and losing that race means cancelling a booking that was paid for a
    /// moment later. A hold that *is* paid after this runs is the gap named below.
    async fn react(
        tx: &Transaction<Client>,
        bookings: &BookingRepository<&Transaction<Client>>,
        event: &SpotEvent,
        at: DateTime<Utc>,
    ) -> MyResult<()> {
        let (spot_id, availability) = match event {
            // A narrowed availability may leave paid bookings outside it. An edit
            // that carries no availability cannot — and the live switch is exactly
            // that edit, which is how flipping a listing off still honours the
            // bookings already made.
            SpotEvent::Updated(e) => match &e.availability {
                Some(a) => (e.spot_id, Some(a)),
                None => return Ok(()),
            },
            // The host says they cannot provide the space at all: everything still
            // owed goes, no check needed.
            SpotEvent::Deleted { spot_id } => (*spot_id, None),
            // Created has no bookings yet.
            _ => return Ok(()),
        };

        // ponytail: reads the booking table, which the *other* projector writes on
        // its own cursor, and this event is never redelivered — so a booking that
        // becomes confirmed after this point keeps hours the host has withdrawn.
        // Two windows: milliseconds, for a payment confirmed just before the edit but
        // not yet projected; and up to `HOLD`, for a live hold paid after it, since
        // `confirm_paid` publishes unconditionally and re-checks nothing.
        // Close both with a reconciliation sweep over confirmed future bookings,
        // shaped like sweeper.rs, if it ever shows up in practice.
        for booking in bookings.upcoming_confirmed(&spot_id, at).await? {
            if let Some(availability) = availability
                && fits(availability, &booking)
            {
                continue;
            }
            Self::cancel(tx, bookings, &booking, at).await?;
        }
        Ok(())
    }

    /// Writes the cancellation **and** enqueues its event, in the caller's
    /// transaction.
    ///
    /// It used to only publish, and booking-service's own projector applied the row
    /// a moment later. That projector is gone — this service writes its own rows
    /// now — so publishing alone left the authoritative booking `confirmed` while
    /// view-service and payment-service both showed it cancelled. Exactly the wrong
    /// way round.
    async fn cancel(
        tx: &Transaction<Client>,
        bookings: &BookingRepository<&Transaction<Client>>,
        booking: &Booking,
        at: DateTime<Utc>,
    ) -> MyResult<()> {
        let version = shared::db::next_version(tx, "booking", &booking.id).await?;

        // Scoped to `confirmed`, so a redelivered SpotUpdated is a no-op — the same
        // guard the projector arm used to carry.
        bookings
            .transition(
                booking.id,
                status::CANCELLED,
                &[status::CONFIRMED],
                None,
                Some(CancelReason::SpotUnavailable.as_str()),
            )
            .await?;
        shared::db::set_version(tx, "booking", &booking.id, version).await?;

        let event = BookingEvent::Cancelled {
            booking_id: booking.id,
            reason: CancelReason::SpotUnavailable,
        };
        // `actor_id: None` — the host acted on the spot, not on this booking.
        let mut envelope = Envelope::new(
            event,
            None,
            aggregate_id("booking", &booking.id),
            version,
        );
        // Deterministic, like the sweeper's: a redelivered SpotUpdated must
        // not publish a second cancel for the same booking. Keyed on the event's own
        // timestamp too, so a *later* edit that invalidates the same booking again
        // is still its own event rather than being swallowed as a duplicate.
        envelope.event_id = Uuid::new_v5(
            &Uuid::NAMESPACE_OID,
            format!("spot-cancel:{}:{}", booking.id, at.timestamp_millis()).as_bytes(),
        );

        tracing::info!(
            booking = %booking.id,
            spot = %booking.spot_id,
            "cancelling: spot can no longer honour it"
        );

        // In the projector's own transaction, so the row and the event commit
        // together. This was the last NATS publish inside a database transaction —
        // the one genuine dual write left from before the rewrite.
        bus::outbox::enqueue(tx, &booking_subject(&booking.spot_id), &envelope).await?;
        Ok(())
    }
}

/// Whether a paid booking still sits inside the spot's hours.
///
/// Nothing is passed as busy: the question is only whether the host is still *open*
/// at these times, and a booking measured against its own slots would collide with
/// itself.
fn fits(availability: &Availability, booking: &Booking) -> bool {
    availability::check(availability, &Booked::new(), &booking.booked).is_ok()
}

// `BookingProjector` is gone. This service's own BOOKINGS events are no longer
// projected back in: `booking_service`, `sweeper` and `payment_worker_service`
// write the `booking` rows directly, inside the transaction that enqueues the
// event. It also advanced `spot.bookings_seq`; that bump now happens in
// `create_booking`'s transaction, where it is the serialisation point rather than
// a cursor.
//
// What remains above is the SPOTS mirror — a *foreign* stream, which is what a
// projector is still for.

#[cfg(test)]
mod tests {
    use super::*;
    use shared::domain_models::booking::status;
    use shared::general_models::spot::{TimeSlot, WeeklyAvailability};
    use std::collections::HashMap;

    fn slot(start: &str, end: &str) -> TimeSlot {
        TimeSlot {
            start: start.into(),
            end: end.into(),
        }
    }

    fn booking(date: &str, slots: Vec<TimeSlot>) -> Booking {
        Booking {
            id: Uuid::now_v7(),
            version: 1,
            spot_id: Uuid::now_v7(),
            owner_id: Uuid::now_v7(),
            renter_id: Uuid::now_v7(),
            booked: HashMap::from([(date.to_string(), slots)]),
            amount: 500,
            status: status::CONFIRMED.into(),
            hold_until: None,
            release_reason: None,
            cancel_reason: None,
            ends_at: Utc::now().into(),
            rating: None,
            created_at: Utc::now().into(),
        }
    }

    /// Monday hours only, so a booking's fate depends on the weekday of its date.
    fn availability(monday: Vec<TimeSlot>) -> Availability {
        Availability {
            weekly: WeeklyAvailability {
                monday,
                tuesday: vec![],
                wednesday: vec![],
                thursday: vec![],
                friday: vec![],
                saturday: vec![],
                sunday: vec![],
            },
            single: HashMap::new(),
        }
    }

    #[test]
    fn only_bookings_outside_the_new_hours_are_cancelled() {
        // 2026-08-03 is a Monday.
        let hours = availability(vec![slot("08:00", "18:00")]);

        // Inside — survives. This is the case that matters: measure a booking against
        // slots that include its own and every edit cancels every booking.
        assert!(fits(
            &hours,
            &booking("2026-08-03", vec![slot("09:00", "10:00")])
        ));
        // Starts inside, runs past the close.
        assert!(!fits(
            &hours,
            &booking("2026-08-03", vec![slot("17:00", "19:00")])
        ));
        // Entirely outside.
        assert!(!fits(
            &hours,
            &booking("2026-08-03", vec![slot("06:00", "07:00")])
        ));
        // Right day, but the host closed Mondays altogether.
        assert!(!fits(
            &availability(vec![]),
            &booking("2026-08-03", vec![slot("09:00", "10:00")])
        ));
        // Two slots, one of them now outside: the whole booking goes.
        assert!(!fits(
            &hours,
            &booking(
                "2026-08-03",
                vec![slot("09:00", "10:00"), slot("19:00", "20:00")]
            )
        ));
    }

    #[test]
    fn a_single_date_entry_overrides_that_weekdays_hours() {
        // Same precedence the reserve path uses, and the reason an edit that only
        // touches one date still has to be checked against every booking on it.
        let mut hours = availability(vec![slot("08:00", "18:00")]);
        hours.single.insert("2026-08-03".into(), vec![]);
        assert!(!fits(
            &hours,
            &booking("2026-08-03", vec![slot("09:00", "10:00")])
        ));
    }
}
