use std::sync::Arc;
use std::time::Duration;

use async_nats::jetstream::Context;
use axum::http::StatusCode;
use bus::{PublishError, Readiness};
use chrono::{DateTime, Utc};
use shared::{
    error::myerror::{ContextExt, MyError, MyResult},
    events::{
        Envelope, STREAM_BOOKINGS,
        booking::{BookingEvent, BookingReserved, ReleaseReason},
        booking_subject, user::record_key,
    },
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
        renter_id: String,
    ) -> MyResult<Reserved> {
        let spot_key = request.spot_id.clone();
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
            if let Some(existing) = self
                .repository
                .booking_for_update(&record_key(&booking_id))
                .await?
            {
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

            let expires_at = Utc::now() + HOLD;
            let event = BookingEvent::Reserved(BookingReserved {
                booking_id,
                spot_id: parse_uuid(&spot_key)?,
                spot_shard: shard.clone(),
                owner_id: owner_id.clone(),
                renter_id: renter_id.clone(),
                booked: requested.clone(),
                amount_cents,
                expires_at,
            });

            match self
                .publish(&spot_key, shard, &event, &renter_id, Some(spot.bookings_seq))
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

    /// Payment succeeded. Stands in for the provider callback until one exists.
    pub async fn confirm(&self, booking_id: &str, renter_id: &str) -> MyResult<u64> {
        self.transition(booking_id, renter_id, |id| BookingEvent::Confirmed {
            booking_id: id,
        })
        .await
    }

    /// The renter backed out of checkout. Frees the slots immediately rather than
    /// waiting out the hold.
    pub async fn release(&self, booking_id: &str, renter_id: &str) -> MyResult<u64> {
        self.transition(booking_id, renter_id, |id| BookingEvent::Released {
            booking_id: id,
            reason: ReleaseReason::Abandoned,
        })
        .await
    }

    /// Shared confirm/release path: authorize, re-check, publish under CAS.
    async fn transition(
        &self,
        booking_id: &str,
        renter_id: &str,
        event: impl Fn(Uuid) -> BookingEvent,
    ) -> MyResult<u64> {
        let id = parse_uuid(booking_id)?;

        for _ in 0..ATTEMPTS {
            let booking = self
                .repository
                .booking_for_update(booking_id)
                .await?
                .context_not_found(("Not Found", "That booking doesn't exist."))?;

            authorize(&booking, renter_id)?;
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
        let Some(availability) = spot.availability.as_ref() else {
            return Ok(());
        };
        availability::check(availability, &spot.booked, &booking.booked, Some(&booking.booked))
            .map_err(reject)?;
        Ok(())
    }

    async fn publish(
        &self,
        spot_key: &str,
        shard: &str,
        event: &BookingEvent,
        actor: &str,
        expected: Option<u64>,
    ) -> Result<u64, PublishError> {
        let spot_id = parse_uuid(spot_key).map_err(PublishError::Failed)?;
        bus::publish_expecting(
            &self.js,
            booking_subject(shard, &spot_id),
            &Envelope::new(event.clone(), Some(actor.to_string())),
            expected,
        )
        .await
    }

    /// Waits for this instance's projector to reach the subject's current head.
    ///
    /// Without this the retry re-reads the same stale projection, asserts the same
    /// stale sequence, and is refused again — a loop that can never converge.
    async fn catch_up(&self, spot_key: &str, shard: &str) -> MyResult<()> {
        let spot_id = parse_uuid(spot_key)?;
        let head = bus::subject_head(&self.js, STREAM_BOOKINGS, &booking_subject(shard, &spot_id))
            .await?;
        // Not a warning: losing a CAS race is the mechanism working, not a fault.
        // Logged because it's otherwise invisible, and a spot generating a steady
        // stream of these is the signal that ATTEMPTS needs backoff.
        tracing::info!(spot = %spot_key, head, "lost CAS race; catching up before retry");

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

fn authorize(booking: &BookingForUpdate, renter_id: &str) -> MyResult<()> {
    // Ownership from the verified token, never from the request.
    if booking.renter_id != renter_id {
        // 404 rather than 403: whether a booking id exists isn't this caller's
        // business.
        return Err(MyError::api(
            StatusCode::NOT_FOUND,
            "Not Found",
            "That booking doesn't exist.",
        ));
    }
    if booking.status != "reserved" {
        return Err(MyError::api(
            StatusCode::CONFLICT,
            "Already settled",
            format!("This booking is already {}.", booking.status),
        ));
    }
    Ok(())
}

fn parse_uuid(key: &str) -> MyResult<Uuid> {
    Uuid::parse_str(key).context_bad_request(("Bad Request", "Malformed id."))
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
