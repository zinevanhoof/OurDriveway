use std::sync::Arc;

use async_nats::jetstream::Context;
use bus::Projector;
use chrono::{DateTime, Utc};
use shared::{
    error::myerror::{MyError, MyResult},
    events::{
        Envelope, STREAM_BOOKINGS, STREAM_SPOTS,
        booking::{BookingEvent, CancelReason},
        booking_subject,
        spot::SpotEvent,
    },
    general_models::spot::Availability,
};
use uuid::Uuid;

use crate::repository::booking_repository::{BookingRepository, LiveBooking};
use crate::service::availability;

/// booking-service consumes SPOTS as well as its own stream: it needs price,
/// availability and active-ness to authorize and price a booking server-side, and
/// the spot table lives in another service's database.
///
/// The two advance independently, which is exactly why a BOOKINGS event can arrive
/// for a spot this projector hasn't created yet — see the `option<>` fields in
/// booking-schema.surql.
pub struct SpotProjector {
    pub repository: Arc<BookingRepository>,
    /// This projector publishes as well as projects — see `react`.
    pub js: Context,
}

impl Projector for SpotProjector {
    const STREAM: &'static str = STREAM_SPOTS;

    async fn last_seq(&self) -> MyResult<u64> {
        self.repository.last_seq(STREAM_SPOTS).await
    }

    async fn apply(&self, payload: &[u8], seq: u64) -> MyResult<()> {
        let envelope: Envelope<SpotEvent> = serde_json::from_slice(payload)
            .map_err(|e| MyError::Bus(format!("decode SpotEvent at seq {seq}: {e}")))?;

        // React *before* projecting. `apply_spot` advances the SPOTS cursor in the
        // same transaction as the row, so a publish failing after it would never be
        // retried — this event is not redelivered once the cursor has moved past it.
        // In this order a failure leaves the cursor where it was and the whole thing
        // runs again; the cancels are deduped by their event ids, and the projection
        // write is an idempotent UPSERT.
        //
        // Nothing is read from the projection that the event doesn't already carry,
        // so running first costs nothing in accuracy.
        self.react(&envelope).await?;
        self.repository.apply_spot(envelope, seq).await
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
    /// Only *confirmed* bookings are touched. A live hold on removed slots can't be
    /// confirmed anyway (`BookingService::recheck`) and lapses within `HOLD`, so
    /// releasing it here would buy fifteen minutes at the cost of racing a payment.
    async fn react(&self, envelope: &Envelope<SpotEvent>) -> MyResult<()> {
        let (spot_id, availability) = match &envelope.payload {
            // A narrowed availability may leave paid bookings outside it.
            SpotEvent::Updated(e) => match &e.availability {
                Some(a) => (e.spot_id, Some(a)),
                None => return Ok(()),
            },
            // The host says they cannot provide the space at all: everything still
            // owed goes, no check needed.
            SpotEvent::Deleted { spot_id } => (*spot_id, None),
            // Created has no bookings yet; the live switch deliberately honours the
            // ones already made.
            _ => return Ok(()),
        };

        let at = envelope.occurred_at;

        // ponytail: reads the booking table, which the *other* projector writes on
        // its own cursor. A booking confirmed moments before this edit may not be
        // projected yet, and this event is never redelivered — so it would keep a
        // booking outside the host's new hours. Milliseconds wide, and the reverse
        // order is safe (a confirm arriving after this is refused by `recheck`).
        // Close it with a reconciliation sweep over confirmed future bookings,
        // shaped like service/expiry.rs, if it ever shows up in practice.

        for booking in self.repository.upcoming_confirmed(&spot_id, at).await? {
            if let Some(availability) = availability
                && fits(availability, &booking)
            {
                continue;
            }
            self.cancel(&booking.spot_shard, &spot_id, &booking.id, at)
                .await?;
        }
        Ok(())
    }

    async fn cancel(
        &self,
        spot_shard: &str,
        spot_id: &Uuid,
        booking_id: &Uuid,
        at: DateTime<Utc>,
    ) -> MyResult<()> {
        let event = BookingEvent::Cancelled {
            booking_id: *booking_id,
            reason: CancelReason::SpotUnavailable,
        };
        // `actor_id: None` — the host acted on the spot, not on this booking.
        let mut envelope = Envelope::new(event, None);
        // Deterministic, like the expiry sweeper's: a redelivered SpotUpdated must
        // not publish a second cancel for the same booking. Keyed on the event's own
        // timestamp too, so a *later* edit that invalidates the same booking again
        // is still its own event rather than being swallowed as a duplicate.
        envelope.event_id = Uuid::new_v5(
            &Uuid::NAMESPACE_OID,
            format!("spot-cancel:{booking_id}:{}", at.timestamp_millis()).as_bytes(),
        );

        tracing::info!(%booking_id, spot = %spot_id, "cancelling: spot can no longer honour it");

        // No compare-and-swap: a cancel only ever frees slots, so there is no race it
        // can lose in a way that matters — the same reasoning as `BookingService::cancel`.
        bus::publish(&self.js, booking_subject(spot_shard, spot_id), &envelope).await?;
        Ok(())
    }
}

/// Whether a paid booking still sits inside the spot's hours.
///
/// `skip` is the booking's own slots: they are in `spot.booked` already, and without
/// excluding them every booking would read as colliding with itself. What's left is
/// exactly the question being asked — is it still *open* at these times.
fn fits(availability: &Availability, booking: &LiveBooking) -> bool {
    availability::check(
        availability,
        &booking.booked,
        &booking.booked,
        Some(&booking.booked),
    )
    .is_ok()
}

pub struct BookingProjector {
    pub repository: Arc<BookingRepository>,
}

impl Projector for BookingProjector {
    const STREAM: &'static str = STREAM_BOOKINGS;

    async fn last_seq(&self) -> MyResult<u64> {
        self.repository.last_seq(STREAM_BOOKINGS).await
    }

    async fn apply(&self, payload: &[u8], seq: u64) -> MyResult<()> {
        let envelope: Envelope<BookingEvent> = serde_json::from_slice(payload)
            .map_err(|e| MyError::Bus(format!("decode BookingEvent at seq {seq}: {e}")))?;
        self.repository.apply_booking(envelope, seq).await
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

    fn booking(date: &str, slots: Vec<TimeSlot>) -> LiveBooking {
        LiveBooking {
            id: Uuid::now_v7(),
            spot_shard: "00".into(),
            booked: HashMap::from([(date.to_string(), slots)]),
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

        // Inside — survives. This is the case that matters: a booking colliding with
        // *itself* through `spot.booked` would cancel every booking on every edit.
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
