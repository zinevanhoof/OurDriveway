use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use bus::outbox;
use chrono::Utc;
use shared::db;
use shared::{
    domain_models::booking::{Booking, status},
    error::myerror::{ContextExt, MyError, MyResult},
    events::{
        Envelope, aggregate_id, format_version,
        booking::{BookingCreated, BookingEvent, CancelReason, ReleaseReason},
        booking_subject,
    },
    requests::booking::CreateBookingRequest,
    responses::booking::CreatedResponse,
};
use surrealdb::{Surreal, engine::remote::ws::Client};
use uuid::Uuid;

use crate::policy::{
    access::{NOT_FOUND, authorize},
    availability::{self, Rejection},
    replay, schedule,
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

/// Write side. Validates, writes its own rows, and enqueues the event beside them
/// — all in one transaction.
///
/// The database is authoritative: this commit *is* the write, and the event in
/// `_outbox` is a durable side effect of the same commit that the relay carries to
/// NATS afterwards. It used to be the other way round — publish, and let this
/// service's own projector apply the row a moment later.
///
/// Every method takes the caller's id first, then what it is acting on, then the
/// request body. The id comes from the verified JWT and never from the body.
pub struct BookingService {
    pub bookings: BookingRepository,
    /// Read-only here, and only for the four columns reserve needs plus the
    /// compare-and-swap cursor — see
    /// [`shared::domain_models::booking::SpotMirror`].
    pub spots: SpotMirrorRepository,
}

impl BookingService {
    pub fn new(db: Arc<Surreal<Client>>) -> Self {
        Self {
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
            let tx = db::begin(&self.bookings.q).await?;
            let bookings = BookingRepository { q: &tx };
            let spots = SpotMirrorRepository { q: &tx };

            // Everything below reads and writes inside ONE transaction, which is
            // what the compare-and-swap on the NATS subject used to buy. The read
            // order no longer matters for correctness the way it did — the version
            // bump at the end is what two racers collide on — but the spot is still
            // read first because the rest of the decision hangs off it.
            let spot = spots
                .find_by_id(spot_key)
                .await?
                .context_not_found(("Not Found", "That spot doesn't exist."))?;

            // A previous attempt committed and we simply never heard back. Because
            // `booking_id` is stable across attempts this is recognisable, and the
            // renter gets their booking instead of a second one.
            if let Some(existing) = bookings.find_by_id(booking_id).await? {
                tx.cancel().await?;
                return Ok(CreatedResponse {
                    id: booking_id,
                    seq: format_version(&aggregate_id("booking", &booking_id), existing.version),
                });
            }

            // The SPOTS mirror may not have caught up with this spot yet. Fail
            // closed rather than booking against an availability we cannot see.
            let (availability, price, owner_id) = spot.bookable().context_conflict(NOT_READY)?;

            spot.active
                .context_conflict(("Unavailable", "This spot is no longer accepting bookings."))?;
            (owner_id != *renter_id)
                .context_unprocessable_entity(("Not allowed", "You can't book your own spot."))?;

            let taken = bookings.taken_for_spot(&spot_key, Utc::now()).await?;
            let minutes = availability::check(availability, &taken, &requested).map_err(reject)?;

            // Truncating division rounds in the renter's favour. Slots are on a
            // 30-minute grid, so it only bites on a hand-crafted request.
            let amount_cents = minutes * price / 60;

            // Folded here rather than by a reader: the zone is a spot field, and
            // "is it over yet" cannot be asked of wall-clock strings without it.
            let ends_at = spot
                .timezone
                .as_deref()
                .and_then(|tz| schedule::ends_at(&requested, tz))
                .context_unprocessable_entity(NOT_READY)?;

            let created = BookingCreated {
                booking_id,
                spot_id: spot_key,
                owner_id,
                renter_id: *renter_id,
                booked: requested.clone(),
                amount_cents,
                expires_at: Utc::now() + HOLD,
                ends_at,
            };

            bookings
                .upsert(Booking::created(created.clone(), Utc::now(), 1))
                .await?;

            // THE serialisation point, and the whole reason this is safe. Two
            // renters racing the same slot both read the same free availability and
            // both insert a *different* booking row — different keys, no conflict.
            // Bumping the spot's counter makes them write one shared key, and TiKV
            // refuses one of them. Verified in `bus/examples/tikv_spike.rs`, which
            // reproduces the double-booking without this line.
            spots.advance(&spot_key, spot.bookings_seq + 1).await?;

            let envelope = Envelope::new(
                BookingEvent::Created(created),
                Some(*renter_id),
                aggregate_id("booking", &booking_id),
                1,
            );
            outbox::enqueue(&tx, &booking_subject(&spot_key), &envelope).await?;

            match tx.commit().await {
                Ok(_) => {
                    return Ok(CreatedResponse {
                        id: booking_id,
                        // The booking's own version, not the spot counter that
                        // serialised it.
                        seq: format_version(&aggregate_id("booking", &booking_id), 1),
                    });
                }
                // Somebody else booked this spot first. Not an error: re-read and
                // decide again against what they left behind — which may well be a
                // clean 409 naming the slot they took.
                Err(e) => {
                    let e = MyError::from(e);
                    if !db::is_write_conflict(&e) {
                        return Err(e);
                    }
                    tracing::info!(spot = %spot_key, "lost a reserve race; retrying");
                }
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
    pub async fn release(&self, renter_id: &Uuid, booking_id: &Uuid) -> MyResult<String> {
        let booking = self
            .bookings
            .find_by_id(*booking_id)
            .await?
            .context_not_found(NOT_FOUND)?;

        authorize(&booking, renter_id, status::RESERVED)?;

        self.settle(
            booking.spot_id,
            *booking_id,
            renter_id,
            status::RELEASED,
            &[status::RESERVED],
            Some(ReleaseReason::Abandoned.as_str()),
            None,
            BookingEvent::Released {
                booking_id: *booking_id,
                reason: ReleaseReason::Abandoned,
            },
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
    pub async fn cancel(&self, renter_id: &Uuid, booking_id: &Uuid) -> MyResult<String> {
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

        self.settle(
            booking.spot_id,
            *booking_id,
            renter_id,
            status::CANCELLED,
            &[status::CONFIRMED],
            None,
            Some(CancelReason::ByRenter.as_str()),
            BookingEvent::Cancelled {
                booking_id: *booking_id,
                reason: CancelReason::ByRenter,
            },
        )
        .await
    }

    /// The shared tail of `release` and `cancel`: move the row, enqueue the event,
    /// commit.
    ///
    /// One helper because the two differ only in which status they move to and from
    /// — and the `from` list is the guard that makes each idempotent. A payment
    /// landing microseconds before a hold lapses must not undo the confirmation,
    /// which is the whole reason the transition is scoped rather than blind.
    ///
    /// Neither needs the spot's version bumped: both only ever *free* slots, so
    /// there is no race against another renter that could produce a double booking.
    #[allow(clippy::too_many_arguments)]
    async fn settle(
        &self,
        spot_id: Uuid,
        booking_id: Uuid,
        renter_id: &Uuid,
        to: &str,
        from: &[&str],
        release: Option<&str>,
        cancel: Option<&str>,
        event: BookingEvent,
    ) -> MyResult<String> {
        let tx = db::begin(&self.bookings.q).await?;
        let version = db::next_version(&tx, "booking", &booking_id).await?;

        BookingRepository { q: &tx }
            .transition(booking_id, to, from, release, cancel)
            .await?;
        db::set_version(&tx, "booking", &booking_id, version).await?;

        let envelope = Envelope::new(
            event,
            Some(*renter_id),
            aggregate_id("booking", &booking_id),
            version,
        );
        let await_token = format_version(&envelope.aggregate, envelope.version);

        outbox::enqueue(&tx, &booking_subject(&spot_id), &envelope).await?;
        tx.commit().await?;

        Ok(await_token)
    }

    /// Re-emits every booking as the events that reproduce its current row, for a
    /// consumer that needs rebuilding. See [`outbox::backfill`] for what this is and
    /// is not.
    ///
    /// One to three events each: a booking's state is reached by a *chain*, and
    /// [`replay::events`] is what knows which one — including why a lone `Cancelled`
    /// would land on nothing.
    ///
    /// This is the stream with side effects on the other end. payment-service's
    /// worker wakes on the two terminal events and calls `settle_up`, which decides
    /// from the payment's *current* status rather than from the event that woke it —
    /// so a booking already refunded yields no second refund. That is a property of
    /// `settle_up`, not of the backfill; check it still holds before adding a
    /// consumer that reacts to these directly.
    pub async fn backfill(&self) -> MyResult<usize> {
        let mut sent = 0;

        for booking in self.bookings.all().await? {
            // One timestamp for the whole chain. Unlike a spot's, a booking's row
            // keeps no record of when it settled — `created_at` is the only clock
            // there is, and nothing downstream stores a settlement time anyway.
            let at = booking.created_at.clone().into();

            sent += outbox::backfill(
                &self.bookings.q,
                &booking_subject(&booking.spot_id),
                &aggregate_id("booking", &booking.id),
                booking.version,
                replay::events(&booking).into_iter().map(|e| (at, e)),
            )
            .await?;
        }

        tracing::info!(events = sent, "bookings backfilled");
        Ok(sent)
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
