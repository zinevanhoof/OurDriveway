use async_nats::jetstream::Context;
use bus::Projector;
use chrono::{DateTime, Utc};
use shared::{
    domain_models::booking::{Booking, SpotMirrorPatch, status},
    error::myerror::MyResult,
    events::{
        Envelope, STREAM_BOOKINGS, STREAM_SPOTS,
        booking::{BookingEvent, CancelReason},
        booking_subject,
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
/// Holds no connection: `bus::Tx` hands it a `&Transaction` per event. It does hold
/// a NATS context, because it publishes as well as projects — see `react`.
pub struct SpotProjector {
    pub js: Context,
}

impl Projector for SpotProjector {
    const STREAM: &'static str = STREAM_SPOTS;
    type Event = SpotEvent;

    async fn apply(
        &self,
        tx: &Transaction<Client>,
        event: SpotEvent,
        at: DateTime<Utc>,
        _seq: u64,
    ) -> MyResult<()> {
        let bookings = BookingRepository { q: tx };
        let spots = SpotMirrorRepository { q: tx };

        // React before projecting. Both now sit inside one transaction, so on any
        // failure the cursor stays put and the whole thing runs again — the cancels
        // are deduped by their deterministic event ids and the mirror write is an
        // idempotent merge.
        //
        // Nothing is read from the projection that the event doesn't already carry,
        // so running first costs nothing in accuracy.
        self.react(&bookings, &event, at).await?;

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
        }
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
        &self,
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
            self.cancel(&booking, at).await?;
        }
        Ok(())
    }

    async fn cancel(&self, booking: &Booking, at: DateTime<Utc>) -> MyResult<()> {
        let event = BookingEvent::Cancelled {
            booking_id: booking.id,
            reason: CancelReason::SpotUnavailable,
        };
        // `actor_id: None` — the host acted on the spot, not on this booking.
        let mut envelope = Envelope::new(event, None);
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

        // No compare-and-swap: a cancel only ever frees slots, so there is no race it
        // can lose in a way that matters — the same reasoning as `BookingService::cancel`.
        bus::publish(
            &self.js,
            booking_subject(&booking.spot_shard, &booking.spot_id),
            &envelope,
        )
        .await?;
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

/// Applies this service's own stream.
///
/// Every arm ends the same way: move that spot's compare-and-swap cursor to this
/// event's sequence, in the transaction that wrote the row.
pub struct BookingProjector;

impl Projector for BookingProjector {
    const STREAM: &'static str = STREAM_BOOKINGS;
    type Event = BookingEvent;

    async fn apply(
        &self,
        tx: &Transaction<Client>,
        event: BookingEvent,
        at: DateTime<Utc>,
        seq: u64,
    ) -> MyResult<()> {
        let bookings = BookingRepository { q: tx };
        let spots = SpotMirrorRepository { q: tx };

        let spot_id = match event {
            BookingEvent::Created(e) => {
                let spot_id = e.spot_id;
                bookings.upsert(Booking::created(e, at)).await?;
                Some(spot_id)
            }

            // Scoped to 'reserved' so a duplicate delivery can't resurrect a booking
            // that was since released.
            BookingEvent::Confirmed { booking_id } => {
                bookings
                    .transition(
                        booking_id,
                        status::CONFIRMED,
                        &[status::RESERVED],
                        None,
                        None,
                    )
                    .await?
            }

            // Only a *reserved* booking can be released. A payment landing
            // microseconds before the hold lapses, with the sweeper's event arriving
            // second, must not undo the confirmation — that WHERE clause is the
            // whole guard.
            BookingEvent::Released { booking_id, reason } => {
                bookings
                    .transition(
                        booking_id,
                        status::RELEASED,
                        &[status::RESERVED],
                        Some(reason.as_str()),
                        None,
                    )
                    .await?
            }

            // Only a *confirmed* booking can be cancelled, so a redelivery after the
            // booking was settled some other way is a no-op — which is also what
            // makes `react` safe to re-run on a replayed SpotUpdated.
            BookingEvent::Cancelled { booking_id, reason } => {
                bookings
                    .transition(
                        booking_id,
                        status::CANCELLED,
                        &[status::CONFIRMED],
                        None,
                        Some(reason.as_str()),
                    )
                    .await?
            }
        };

        // `None` means the booking row does not exist, which is the one case with
        // nothing to point the cursor at. A transition whose guard *refused* still
        // yields its spot id, deliberately: that event consumed a subject sequence
        // either way, and leaving the cursor behind the subject head would refuse
        // every later reserve on the spot.
        let Some(spot_id) = spot_id else {
            return Ok(());
        };

        // Same transaction as the row written above, so the cursor can never run
        // ahead of what reserve reads.
        spots.advance(&spot_id, seq).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
            spot_id: Uuid::now_v7(),
            spot_shard: "00".into(),
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
