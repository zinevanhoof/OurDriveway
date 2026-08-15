use std::sync::Arc;
use std::time::Duration;

use async_nats::jetstream::Context;
use axum::http::StatusCode;
use bus::{PublishError, Readiness};
use chrono::{DateTime, NaiveDateTime, TimeDelta, TimeZone, Utc};
use chrono_tz::Tz;
use shared::{
    error::myerror::{ContextExt, MyError, MyResult},
    events::{
        Envelope, STREAM_BOOKINGS,
        booking::{BookingEvent, BookingReserved, CancelReason, ReleaseReason},
        booking_subject,
    },
    general_models::booking::Booked,
    general_models::spot::TimeSlot,
    requests::booking::CreateBookingRequest,
};
use tokio::sync::watch;
use uuid::Uuid;

use crate::repository::booking_repository::{BookingForUpdate, BookingRepository};
use crate::service::availability::{self, Rejection};

/// How long a checkout holds its slots.
///
/// Long enough for a card form plus a 3-D Secure detour, short enough that an
/// abandoned checkout frees a popular slot quickly.
pub const HOLD: Duration = Duration::from_secs(15 * 60);

/// Attempts before giving up and telling the renter to try again.
///
/// Contention is per-spot and rare. The bound exists because compare-and-swap
/// guarantees safety, not liveness: under real contention this could spin.
const ATTEMPTS: usize = 3;

/// How long to wait for our own projector to catch up after losing a CAS race.
/// Same cap as view-service's `await_seq` — proceeding slightly stale beats
/// hanging a request, and a genuinely stuck projector is already out of rotation.
const CATCH_UP: Duration = Duration::from_secs(2);

/// Write side. Validates and publishes — it never writes to the database.
pub struct BookingService {
    pub js: Context,
    pub repository: Arc<BookingRepository>,
    /// Applied-sequence watch for BOOKINGS, used to wait out a lost CAS race.
    pub applied: watch::Receiver<u64>,
}

pub struct Reserved {
    pub booking_id: Uuid,
    pub seq: u64,
    pub expires_at: DateTime<Utc>,
    pub amount_cents: i64,
}

impl BookingService {
    pub fn new(js: Context, repository: Arc<BookingRepository>, readiness: &Readiness) -> Self {
        Self {
            applied: readiness
                .applied_rx(STREAM_BOOKINGS)
                .expect("BOOKINGS registered with Readiness"),
            js,
            repository,
        }
    }

    /// Holds slots for checkout.
    ///
    /// The overlap check below is advisory — it turns a lost race into a clean 409
    /// naming the slot. The actual guarantee is the compare-and-swap on publish:
    /// two instances can both read a synced projection and both decide to write,
    /// and the server lets exactly one of them append.
    pub async fn reserve(
        &self,
        request: CreateBookingRequest,
        renter_id: Uuid,
    ) -> MyResult<Reserved> {
        let spot_key = request.spot_id;
        let requested = request.slots();

        // Stable across attempts: it's the idempotency key. If an ack is lost after
        // the event landed, the retry sees this booking already projected and
        // returns success instead of double-booking the renter.
        let booking_id = Uuid::now_v7();

        for _ in 0..ATTEMPTS {
            let spot = self
                .repository
                .spot_for_booking(&spot_key)
                .await?
                .context_not_found(("Not Found", "That spot doesn't exist."))?;

            // A previous attempt's event landed and we simply never heard the ack.
            // Because `booking_id` is stable across attempts this is recognisable,
            // and the renter gets their reservation instead of a second one.
            if let Some(existing) = self.repository.booking_for_update(&booking_id).await? {
                return Ok(Reserved {
                    booking_id,
                    // The spot's cursor is at or past our own event by definition —
                    // it was advanced by projecting it — so it's a safe position for
                    // the client to wait on.
                    seq: spot.bookings_seq,
                    expires_at: existing.hold_until.unwrap_or_else(Utc::now),
                    amount_cents: existing.amount,
                });
            }

            // The SPOTS projection may not have caught up with this spot yet. Fail
            // closed rather than booking against an availability we can't see.
            let (Some(availability), Some(price), Some(owner_id), Some(shard)) = (
                spot.availability.as_ref(),
                spot.price_per_hour,
                spot.owner_id.as_ref(),
                spot.shard.as_ref(),
            ) else {
                return Err(MyError::api(
                    StatusCode::CONFLICT,
                    "Not bookable yet",
                    "This spot isn't ready to accept bookings. Try again in a moment.",
                ));
            };

            if !spot.active {
                return Err(MyError::api(
                    StatusCode::CONFLICT,
                    "Unavailable",
                    "This spot is no longer accepting bookings.",
                ));
            }
            if *owner_id == renter_id {
                return Err(MyError::api(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "Not allowed",
                    "You can't book your own spot.",
                ));
            }

            let minutes = availability::check(availability, &spot.booked, &requested, None)
                .map_err(reject)?;

            // Truncating division rounds in the renter's favour. Slots are on a
            // 30-minute grid, so it only bites on a hand-crafted request.
            let amount_cents = minutes * price / 60;

            // Folded here rather than by each projector: the zone is a spot field,
            // and a projector that had to look it up would be reading state instead
            // of the event. Fails closed for the same reason cancel does — a booking
            // whose end can't be placed on a timeline can't be filtered by one.
            let ends_at = spot
                .timezone
                .as_deref()
                .and_then(|tz| ends_at(&requested, tz))
                .context_unprocessable_entity((
                    "Unavailable",
                    "This spot isn't ready to accept bookings. Try again in a moment.",
                ))?;

            let expires_at = Utc::now() + HOLD;
            let event = BookingEvent::Reserved(BookingReserved {
                booking_id,
                spot_id: spot_key,
                spot_shard: shard.clone(),
                owner_id: *owner_id,
                renter_id,
                booked: requested.clone(),
                amount_cents,
                expires_at,
                ends_at,
            });

            match self
                .publish(
                    &spot_key,
                    shard,
                    &event,
                    &renter_id,
                    Some(spot.bookings_seq),
                )
                .await
            {
                Ok(seq) => {
                    return Ok(Reserved {
                        booking_id,
                        seq,
                        expires_at,
                        amount_cents,
                    });
                }
                Err(PublishError::Stale) => self.catch_up(&spot_key, shard).await?,
                Err(PublishError::Failed(e)) => return Err(e),
            }
        }

        Err(taken_now())
    }

    /// Payment succeeded, as reported by a signature-verified Stripe webhook.
    ///
    /// Called only by `worker::PaymentWorker`, never from a request. There is
    /// deliberately no renter-facing confirm endpoint: a renter who could confirm their
    /// own booking would not have to pay for it.
    ///
    /// This is *post*-capture, which is what makes it the opposite of `transition` in
    /// every respect that matters — no `authorize`, no `recheck`, no compare-and-swap:
    ///
    /// - Money has already moved. Refusing here would leave a captured payment with no
    ///   booking attached, which is worse than any state this could produce.
    /// - So it publishes unconditionally, and if the booking no longer fits the spot's
    ///   hours, `SpotProjector::react` withdraws it as `SpotUnavailable` — and
    ///   payment-service refunds off that one trigger. One path for money coming back,
    ///   not a second decision made here with half the picture.
    /// - The projector's `WHERE status IN ['reserved']` is what keeps this idempotent:
    ///   a redelivery, or a payment landing after the hold lapsed, applies to nothing.
    ///
    /// `payment_id` only names the event, so a redelivered webhook is discarded by the
    /// stream's duplicate window instead of appending a second `Confirmed`.
    pub async fn confirm_paid(&self, booking_id: Uuid, payment_id: Uuid) -> MyResult<u64> {
        let booking = self
            .repository
            .booking_for_update(&booking_id)
            .await?
            .context_not_found(("Not Found", "That booking doesn't exist."))?;

        let event = BookingEvent::Confirmed { booking_id };

        // `actor_id: None` — Stripe acted, not a user holding a token.
        let mut envelope = Envelope::new(event, None);
        envelope.event_id = Uuid::new_v5(
            &Uuid::NAMESPACE_OID,
            format!("payment-confirm:{payment_id}").as_bytes(),
        );

        Ok(bus::publish(
            &self.js,
            booking_subject(&booking.spot_shard, &booking.spot_id),
            &envelope,
        )
        .await?)
    }

    /// The renter backed out of checkout. Frees the slots immediately rather than
    /// waiting out the hold.
    pub async fn release(&self, booking_id: &Uuid, renter_id: &Uuid) -> MyResult<u64> {
        self.transition(booking_id, renter_id, |id| BookingEvent::Released {
            booking_id: id,
            reason: ReleaseReason::Abandoned,
        })
        .await
    }

    /// The renter withdraws a booking they already paid for.
    ///
    /// Deliberately not `transition`. That path re-checks availability, which is
    /// meaningless here — the slots are already ours and we are handing them back,
    /// so a host who narrowed their hours since would turn a cancel into a 422. It
    /// also publishes under compare-and-swap, which a cancel doesn't need: like the
    /// expiry sweeper it only ever *frees* slots, so it can't lose a race in a way
    /// that matters, and the projector's `WHERE status IN ['confirmed']` makes a
    /// redelivery or a client retry a no-op.
    pub async fn cancel(&self, booking_id: &Uuid, renter_id: &Uuid) -> MyResult<u64> {
        let booking = self
            .repository
            .booking_for_update(booking_id)
            .await?
            .context_not_found(("Not Found", "That booking doesn't exist."))?;
        authorize(&booking, renter_id, "confirmed")?;

        let timezone = self
            .repository
            .spot_for_booking(&booking.spot_id)
            .await?
            .and_then(|s| s.timezone);

        // Fails closed. A spot whose projection hasn't landed, an unknown zone, or
        // times that don't parse all mean we cannot *prove* the cancel is in time —
        // and the host has been holding the space on the strength of this booking.
        if timezone
            .as_deref()
            .and_then(|tz| in_time(&booking.booked, tz, Utc::now()))
            != Some(true)
        {
            return Err(MyError::api(
                StatusCode::CONFLICT,
                "Too late to cancel",
                "A booking can only be cancelled up to an hour before it starts.",
            ));
        }

        let event = BookingEvent::Cancelled {
            booking_id: *booking_id,
            reason: CancelReason::ByRenter,
        };
        match self
            .publish(
                &booking.spot_id,
                &booking.spot_shard,
                &event,
                renter_id,
                None,
            )
            .await
        {
            Ok(seq) => Ok(seq),
            // Unreachable with `expected: None` — there is no assertion to lose.
            Err(PublishError::Stale) => Err(taken_now()),
            Err(PublishError::Failed(e)) => Err(e),
        }
    }

    /// Shared confirm/release path: authorize, re-check, publish under CAS.
    async fn transition(
        &self,
        booking_id: &Uuid,
        renter_id: &Uuid,
        event: impl Fn(Uuid) -> BookingEvent,
    ) -> MyResult<u64> {
        let id = *booking_id;

        for _ in 0..ATTEMPTS {
            let booking = self
                .repository
                .booking_for_update(booking_id)
                .await?
                .context_not_found(("Not Found", "That booking doesn't exist."))?;

            authorize(&booking, renter_id, "reserved")?;
            self.recheck(&booking).await?;

            let seq = self
                .repository
                .spot_for_booking(&booking.spot_id)
                .await?
                .map(|s| s.bookings_seq)
                .unwrap_or(0);

            match self
                .publish(
                    &booking.spot_id,
                    &booking.spot_shard,
                    &event(id),
                    renter_id,
                    Some(seq),
                )
                .await
            {
                Ok(seq) => return Ok(seq),
                Err(PublishError::Stale) => {
                    self.catch_up(&booking.spot_id, &booking.spot_shard).await?
                }
                Err(PublishError::Failed(e)) => return Err(e),
            }
        }

        Err(taken_now())
    }

    /// Re-verifies that a booking's slots are still its own to take.
    ///
    /// Runs on every confirm, not only when the hold lapsed. If the hold was live
    /// nothing could have taken the slots and this passes trivially; if it lapsed
    /// and someone else booked over it, the renter gets a 409 instead of paying for
    /// a slot they no longer have.
    async fn recheck(&self, booking: &BookingForUpdate) -> MyResult<()> {
        let Some(spot) = self.repository.spot_for_booking(&booking.spot_id).await? else {
            return Ok(());
        };
        // A deleted listing can't be confirmed into, even by a hold taken before the
        // deletion. `active` is deliberately *not* checked: flipping the live switch
        // off stops new reservations, and a checkout already under way predates it.
        if spot.deleted {
            return Err(MyError::api(
                StatusCode::CONFLICT,
                "Unavailable",
                "The host withdrew this spot.",
            ));
        }
        let Some(availability) = spot.availability.as_ref() else {
            return Ok(());
        };
        availability::check(
            availability,
            &spot.booked,
            &booking.booked,
            Some(&booking.booked),
        )
        .map_err(reject)?;
        Ok(())
    }

    async fn publish(
        &self,
        spot_id: &Uuid,
        shard: &str,
        event: &BookingEvent,
        actor: &Uuid,
        expected: Option<u64>,
    ) -> Result<u64, PublishError> {
        bus::publish_expecting(
            &self.js,
            booking_subject(shard, spot_id),
            &Envelope::new(event.clone(), Some(*actor)),
            expected,
        )
        .await
    }

    /// Waits for this instance's projector to reach the subject's current head.
    ///
    /// Without this the retry re-reads the same stale projection, asserts the same
    /// stale sequence, and is refused again — a loop that can never converge.
    async fn catch_up(&self, spot_id: &Uuid, shard: &str) -> MyResult<()> {
        let head =
            bus::subject_head(&self.js, STREAM_BOOKINGS, &booking_subject(shard, spot_id)).await?;
        // Not a warning: losing a CAS race is the mechanism working, not a fault.
        // Logged because it's otherwise invisible, and a spot generating a steady
        // stream of these is the signal that ATTEMPTS needs backoff.
        tracing::info!(spot = %spot_id, head, "lost CAS race; catching up before retry");

        let mut applied = self.applied.clone();
        let _ = tokio::time::timeout(CATCH_UP, async {
            while *applied.borrow() < head {
                if applied.changed().await.is_err() {
                    break;
                }
            }
        })
        .await;
        Ok(())
    }
}

fn authorize(booking: &BookingForUpdate, renter_id: &Uuid, expected: &str) -> MyResult<()> {
    // Ownership from the verified token, never from the request.
    if booking.renter_id != *renter_id {
        // 404 rather than 403: whether a booking id exists isn't this caller's
        // business.
        return Err(MyError::api(
            StatusCode::NOT_FOUND,
            "Not Found",
            "That booking doesn't exist.",
        ));
    }
    // A parameter, not a constant: confirm and release leave `reserved`, cancel
    // leaves `confirmed`.
    if booking.status != expected {
        return Err(MyError::api(
            StatusCode::CONFLICT,
            "Already settled",
            format!("This booking is already {}.", booking.status),
        ));
    }
    Ok(())
}

/// How long before a booking starts cancelling closes. Any shorter and the host
/// is already standing in the driveway.
const CUTOFF: TimeDelta = TimeDelta::hours(1);

/// Whether a cancel at `now` is still in time.
///
/// `None` when the start can't be determined at all — an unknown zone, or times
/// that don't parse. The caller treats that as "no", because we can't hand out a
/// cancel we can't prove is in time.
///
/// `now` is a parameter rather than a clock read so this stays pure and testable.
fn in_time(booked: &Booked, timezone: &str, now: DateTime<Utc>) -> Option<bool> {
    Some(now + CUTOFF <= starts_at(booked, timezone)?)
}

/// The first moment a booking occupies, as a UTC instant.
///
/// `booked` is `"YYYY-MM-DD"` plus `"HH:MM"` with no zone attached — it is the
/// *spot's* wall clock. Reading it as UTC would slide an hour-long deadline by the
/// spot's whole offset, which in Brussels means two hours the wrong way in summer.
///
/// The minimum is taken across every date and every slot: `booked` is a `HashMap`,
/// so the first one iterated is not the first one that happens.
fn starts_at(booked: &Booked, timezone: &str) -> Option<DateTime<Utc>> {
    let tz: Tz = timezone.parse().ok()?;
    let first = wall_times(booked, |s| &s.start).min()?;
    instant(first, tz)
}

/// The last moment a booking occupies, as a UTC instant.
///
/// The mirror of `starts_at`, and the field every "is this still to come" filter
/// reads. Same reason for the fold: a `HashMap` of wall-clock strings has no order
/// of its own, so the last date iterated is not the last one that happens.
fn ends_at(booked: &Booked, timezone: &str) -> Option<DateTime<Utc>> {
    let tz: Tz = timezone.parse().ok()?;
    let last = wall_times(booked, |s| &s.end).max()?;
    instant(last, tz)
}

/// Every `"YYYY-MM-DD HH:MM"` in `booked`, picking one end of each slot.
fn wall_times<'a>(
    booked: &'a Booked,
    pick: impl Fn(&TimeSlot) -> &String + Copy + 'a,
) -> impl Iterator<Item = NaiveDateTime> + 'a {
    booked
        .iter()
        .flat_map(move |(date, slots)| slots.iter().map(move |s| format!("{date} {}", pick(s))))
        .filter_map(|local| NaiveDateTime::parse_from_str(&local, "%Y-%m-%d %H:%M").ok())
}

/// A wall time in `tz` as an instant.
///
/// A wall time inside a spring-forward gap names no instant at all. The same time an
/// hour later always does; being an hour stricter one night a year beats a booking
/// that can never be cancelled. `earliest` also settles the autumn ambiguity, in the
/// host's favour — for an end that means a booking leaves the Upcoming tab up to an
/// hour early on that one night, which no money depends on.
fn instant(local: NaiveDateTime, tz: Tz) -> Option<DateTime<Utc>> {
    tz.from_local_datetime(&local)
        .earliest()
        .or_else(|| {
            tz.from_local_datetime(&(local + TimeDelta::hours(1)))
                .earliest()
        })
        .map(|dt| dt.with_timezone(&Utc))
}

fn taken_now() -> MyError {
    MyError::api(
        StatusCode::CONFLICT,
        "Just taken",
        "Someone booked those times a moment ago. Please pick again.",
    )
}

fn reject(rejection: Rejection) -> MyError {
    match rejection {
        Rejection::Closed { date, slot } => MyError::api(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Outside opening hours",
            format!("The host isn't open {}–{} on {date}.", slot.start, slot.end),
        ),
        Rejection::Taken { date, slot } => MyError::api(
            StatusCode::CONFLICT,
            "Already booked",
            format!("{}–{} on {date} is no longer free.", slot.start, slot.end),
        ),
        Rejection::Malformed => MyError::api(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Invalid times",
            "Those time slots don't look right.",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shared::general_models::spot::TimeSlot;
    use std::collections::HashMap;

    fn at(s: &str) -> DateTime<Utc> {
        s.parse().unwrap()
    }

    fn slot(start: &str, end: &str) -> TimeSlot {
        TimeSlot {
            start: start.into(),
            end: end.into(),
        }
    }

    #[test]
    fn cancel_closes_one_hour_before_the_first_slot_in_the_spots_zone() {
        // 09:00 in Brussels on this date is 07:00Z (CEST, UTC+2), so the deadline is
        // 06:00Z. Reading `booked` as bare UTC would put the deadline at 08:00Z and
        // hand out two extra hours of cancelling — the bug this test exists for.
        let booked: Booked = HashMap::from([(
            "2026-08-03".to_string(),
            // Out of order on purpose: `booked` is a HashMap, so "first" has to be a
            // minimum, not whatever happens to iterate first.
            vec![slot("11:00", "12:00"), slot("09:00", "10:00")],
        )]);
        let tz = "Europe/Brussels";

        assert_eq!(in_time(&booked, tz, at("2026-08-03T05:59:59Z")), Some(true));
        // Exactly on the hour still counts — the boundary is inclusive.
        assert_eq!(in_time(&booked, tz, at("2026-08-03T06:00:00Z")), Some(true));
        assert_eq!(
            in_time(&booked, tz, at("2026-08-03T06:00:01Z")),
            Some(false)
        );
        // Inside the naive-UTC window, and correctly refused anyway.
        assert_eq!(
            in_time(&booked, tz, at("2026-08-03T07:30:00Z")),
            Some(false)
        );
        // An earlier date wins over an earlier clock time on a later date: 22:00 on
        // the 3rd is 20:00Z, so the deadline is 19:00Z — not 07:00 on the 4th.
        let spread: Booked = HashMap::from([
            ("2026-08-04".to_string(), vec![slot("08:00", "09:00")]),
            ("2026-08-03".to_string(), vec![slot("22:00", "23:00")]),
        ]);
        assert_eq!(in_time(&spread, tz, at("2026-08-03T19:00:00Z")), Some(true));
        assert_eq!(
            in_time(&spread, tz, at("2026-08-03T19:00:01Z")),
            Some(false)
        );
        // Fails closed: an unknown zone can't be proven in time.
        assert_eq!(
            in_time(&booked, "Not/AZone", at("2026-08-03T05:00:00Z")),
            None
        );
    }

    #[test]
    fn ends_at_is_the_last_moment_across_every_day_in_the_spots_zone() {
        // Deliberately out of order, and spanning two days: `booked` is a HashMap, so
        // "last" has to be a maximum. The 4th's 09:00 iterating first must not win
        // over the 3rd's 23:00 — nor the other way round.
        let booked: Booked = HashMap::from([
            (
                "2026-08-04".to_string(),
                vec![slot("08:00", "09:00"), slot("10:00", "11:00")],
            ),
            ("2026-08-03".to_string(), vec![slot("22:00", "23:00")]),
        ]);

        // 11:00 Brussels on the 4th is 09:00Z in summer. Reading the strings as UTC
        // would answer 11:00Z and keep the booking "upcoming" two hours too long.
        assert_eq!(
            ends_at(&booked, "Europe/Brussels"),
            Some(at("2026-08-04T09:00:00Z"))
        );
        // Same map, other end, so a start/end mix-up can't pass both.
        assert_eq!(
            starts_at(&booked, "Europe/Brussels"),
            Some(at("2026-08-03T20:00:00Z"))
        );
        assert_eq!(ends_at(&booked, "Not/AZone"), None);
        assert_eq!(ends_at(&HashMap::new(), "Europe/Brussels"), None);
    }
}
