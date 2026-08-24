use std::sync::Arc;
use std::time::Duration;

use async_nats::jetstream::Context;
use axum::http::StatusCode;
use bus::{PublishError, Readiness, format_seq};
use chrono::Utc;
use shared::{
    domain_models::booking::status,
    error::myerror::{ContextExt, MyError, MyResult},
    events::{
        Envelope, STREAM_BOOKINGS,
        booking::{BookingCreated, BookingEvent, CancelReason, ReleaseReason},
        booking_subject,
    },
    requests::booking::CreateBookingRequest,
    responses::booking::CreatedResponse,
};
use surrealdb::{Surreal, engine::remote::ws::Client};
use tokio::sync::watch;
use uuid::Uuid;

use crate::policy::{
    access::{NOT_FOUND, authorize},
    availability::{self, Rejection},
    schedule,
};
use crate::repository::{
    booking_repository::BookingRepository, spot_mirror_repository::SpotMirrorRepository,
};

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

/// Answered whenever the spot's mirror is too incomplete to price or authorize
/// against. Fails closed: booking against an availability we cannot see is worse
/// than telling the renter to try again in a moment.
const NOT_READY: (&str, &str) = (
    "Not bookable yet",
    "This spot isn't ready to accept bookings. Try again in a moment.",
);

/// Write side. Validates and publishes — it never writes to the database.
///
/// Every method takes the caller's id first, then what it is acting on, then the
/// request body. The id comes from the verified JWT and never from the body.
pub struct BookingService {
    pub js: Context,
    pub bookings: BookingRepository,
    /// Read-only here, and only for the four columns reserve needs plus the
    /// compare-and-swap cursor — see
    /// [`shared::domain_models::booking::SpotMirror`].
    pub spots: SpotMirrorRepository,
    /// Applied-sequence watch for BOOKINGS, used to wait out a lost CAS race.
    pub applied: watch::Receiver<u64>,
}

impl BookingService {
    pub fn new(js: Context, db: Arc<Surreal<Client>>, readiness: &Readiness) -> Self {
        Self {
            applied: readiness
                .applied_rx(STREAM_BOOKINGS)
                .expect("BOOKINGS registered with Readiness"),
            js,
            bookings: BookingRepository { q: db.clone() },
            spots: SpotMirrorRepository { q: db },
        }
    }

    /// Creates a booking, which starts life as a hold on its slots.
    ///
    /// Named for the booking rather than the hold: reserving the slots is what
    /// creating one *entails*, not a separate thing a client can ask for. The
    /// initial `status` is still `reserved`, because that is what the row is.
    ///
    /// Returns the wire shape directly. The id is in it because the client cannot
    /// get it any other way and needs it immediately — it opens a Stripe Checkout
    /// Session from it before any projection could have caught up. The seq is
    /// formatted here rather than by the route because *this* is what knows the
    /// event went to BOOKINGS; the route only picks the status code.
    ///
    /// The overlap check below is advisory — it turns a lost race into a clean 409
    /// naming the slot. The actual guarantee is the compare-and-swap on publish:
    /// two instances can both read a synced projection and both decide to write,
    /// and the server lets exactly one of them append.
    pub async fn create_booking(
        &self,
        renter_id: &Uuid,
        request: CreateBookingRequest,
    ) -> MyResult<CreatedResponse> {
        let spot_key = request.spot_id;
        let requested = request.booked;

        // Stable across attempts: it's the idempotency key. If an ack is lost after
        // the event landed, the retry sees this booking already projected and
        // returns success instead of double-booking the renter.
        let booking_id = Uuid::now_v7();

        for _ in 0..ATTEMPTS {
            // Read the spot **before** the booking rows below, and keep it that way.
            // The cursor asserted on publish comes from this row; the slots come from
            // rows read after it. So the rows can only be newer than the cursor — a
            // booking that lands in between is seen here (clean 409) while the
            // compare-and-swap asserts the older sequence (refused, retried). Read in
            // the other order and a reserve could authorize against slots it has not
            // seen while asserting a cursor that succeeds, which is a double booking.
            let spot = self
                .spots
                .find_by_id(spot_key)
                .await?
                .context_not_found(("Not Found", "That spot doesn't exist."))?;

            // A previous attempt's event landed and we simply never heard the ack.
            // Because `booking_id` is stable across attempts this is recognisable,
            // and the renter gets their booking instead of a second one.
            if self.bookings.find_by_id(booking_id).await?.is_some() {
                return Ok(CreatedResponse {
                    id: booking_id,
                    // The spot's cursor is at or past our own event by definition —
                    // it was advanced by projecting it — so it's a safe position for
                    // the client to wait on.
                    seq: format_seq(STREAM_BOOKINGS, spot.bookings_seq),
                });
            }

            // The SPOTS projection may not have caught up with this spot yet. Fail
            // closed rather than booking against an availability we can't see.
            let (availability, price, owner_id, shard) =
                spot.bookable().context_conflict(NOT_READY)?;

            spot.active
                .context_conflict(("Unavailable", "This spot is no longer accepting bookings."))?;
            (owner_id != *renter_id)
                .context_unprocessable_entity(("Not allowed", "You can't book your own spot."))?;

            let taken = self.bookings.taken_for_spot(&spot_key, Utc::now()).await?;
            let minutes = availability::check(availability, &taken, &requested).map_err(reject)?;

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
                .and_then(|tz| schedule::ends_at(&requested, tz))
                .context_unprocessable_entity(NOT_READY)?;

            let expires_at = Utc::now() + HOLD;
            let event = BookingEvent::Created(BookingCreated {
                booking_id,
                spot_id: spot_key,
                spot_shard: shard.to_string(),
                owner_id,
                renter_id: *renter_id,
                booked: requested.clone(),
                amount_cents,
                expires_at,
                ends_at,
            });

            match bus::publish_expecting(
                &self.js,
                booking_subject(shard, &spot_key),
                &Envelope::new(event, Some(*renter_id)),
                Some(spot.bookings_seq),
            )
            .await
            {
                Ok(seq) => {
                    return Ok(CreatedResponse {
                        id: booking_id,
                        seq: format_seq(STREAM_BOOKINGS, seq),
                    });
                }
                Err(PublishError::Stale) => {
                    bus::catch_up(
                        &self.js,
                        &self.applied,
                        STREAM_BOOKINGS,
                        &booking_subject(shard, &spot_key),
                    )
                    .await?
                }
                Err(PublishError::Failed(e)) => return Err(e),
            }
        }

        Err(taken_now())
    }

    /// The renter backed out of checkout. Frees the slots immediately rather than
    /// waiting out the hold.
    ///
    /// Authorize, publish. Nothing is re-checked and nothing is asserted, for the
    /// same reason the sweeper needs neither: a release only ever *frees*
    /// slots, so it can't lose a race in a way that matters. `authorize` has already
    /// proved this is the caller's own reserved booking, and the projector's
    /// `WHERE status IN ['reserved']` keeps a redelivery — or a payment that landed
    /// in the same instant — from being undone.
    ///
    /// Deliberately cannot fail on the spot's state. A withdrawn listing or slots
    /// booked over a lapsed hold are both reasons to let the hold go, not to refuse
    /// and leave it blocking the spot for the rest of [`HOLD`].
    pub async fn release(&self, renter_id: &Uuid, booking_id: &Uuid) -> MyResult<u64> {
        let booking = self
            .bookings
            .find_by_id(*booking_id)
            .await?
            .context_not_found(NOT_FOUND)?;

        authorize(&booking, renter_id, status::RESERVED)?;

        let event = BookingEvent::Released {
            booking_id: *booking_id,
            reason: ReleaseReason::Abandoned,
        };
        bus::publish(
            &self.js,
            booking_subject(&booking.spot_shard, &booking.spot_id),
            &Envelope::new(event, Some(*renter_id)),
        )
        .await
    }

    /// The renter withdraws a booking they already paid for.
    ///
    /// Shaped like [`Self::release`] — authorize, then a plain publish — and for the
    /// same reason: like the sweeper both only ever *free* slots, so neither
    /// can lose a race in a way that matters, and the projector's
    /// `WHERE status IN ['confirmed']` makes a redelivery or a client retry a no-op.
    /// Nothing about the spot's current availability is consulted here either: the
    /// slots are already ours and we are handing them back, so a host who narrowed
    /// their hours since would otherwise turn a cancel into a 422.
    ///
    /// The deadline below is the one thing this adds, and it is about the host's
    /// evening rather than about the slots.
    pub async fn cancel(&self, renter_id: &Uuid, booking_id: &Uuid) -> MyResult<u64> {
        let booking = self
            .bookings
            .find_by_id(*booking_id)
            .await?
            .context_not_found(NOT_FOUND)?;
        authorize(&booking, renter_id, status::CONFIRMED)?;

        let timezone = self
            .spots
            .find_by_id(booking.spot_id)
            .await?
            .and_then(|s| s.timezone);

        // Fails closed. A spot whose projection hasn't landed, an unknown zone, or
        // times that don't parse all mean we cannot *prove* the cancel is in time —
        // and the host has been holding the space on the strength of this booking.
        (timezone
            .as_deref()
            .and_then(|tz| schedule::in_time(&booking.booked, tz, Utc::now()))
            == Some(true))
        .context_conflict((
            "Too late to cancel",
            "A booking can only be cancelled up to an hour before it starts.",
        ))?;

        let event = BookingEvent::Cancelled {
            booking_id: *booking_id,
            reason: CancelReason::ByRenter,
        };
        bus::publish(
            &self.js,
            booking_subject(&booking.spot_shard, &booking.spot_id),
            &Envelope::new(event, Some(*renter_id)),
        )
        .await
    }
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
