use bus::Projector;
use chrono::{DateTime, Utc};
use shared::{
    domain_models::{
        booking::status as booking_status,
        payment::{BookingMirror, BookingMirrorPatch},
    },
    error::myerror::MyResult,
    events::{STREAM_BOOKINGS, booking::BookingEvent},
};
use surrealdb::{engine::remote::ws::Client, method::Transaction};

use crate::repository::booking_mirror_repository::BookingMirrorRepository;

/// payment-service consumes BOOKINGS as well as its own stream, for two things it
/// cannot answer otherwise: what a booking costs and who it belongs to (before a
/// payment may be created), and whether it is still going to happen (before money is
/// kept or returned).
///
/// Read-only as far as the outside world is concerned. The refund side effect that
/// *reacts* to these same events is a `Worker`, not this projector — see worker.rs for
/// why that distinction is not optional.
pub struct BookingProjector;

impl Projector for BookingProjector {
    const STREAM: &'static str = STREAM_BOOKINGS;
    /// Not `payment-bookings` — that is [`crate::worker::BookingWorker`]'s, and this
    /// is the one place in the codebase where a projector and a worker read the same
    /// stream. Two consumers, deliberately: the projector delivers from sequence 1 to
    /// every replica, the worker delivers new messages to one. Sharing a name asks
    /// JetStream for both at once, and it refuses — "deliver policy can not be
    /// updated" — leaving whichever lost the race permanently stalled.
    const DURABLE: &'static str = "payment-booking-mirror";
    type Event = BookingEvent;

    async fn apply(
        &self,
        tx: &Transaction<Client>,
        event: BookingEvent,
        _at: DateTime<Utc>,
        version: u64,
    ) -> MyResult<()> {
        let bookings = BookingMirrorRepository { q: tx };
        let booking_id = event.booking_id();

        match event {
            // `upsert`, not `merge`: BookingCreated is always the first event for a
            // booking and BOOKINGS never expires, so this row is only ever created
            // whole — which is also why nothing on this table is `option<>`.
            BookingEvent::Created(e) => bookings.upsert(BookingMirror::created(e)).await,

            BookingEvent::Confirmed { booking_id } => {
                bookings
                    .transition(
                        booking_id,
                        &[booking_status::RESERVED],
                        BookingMirrorPatch::status(booking_status::CONFIRMED),
                    )
                    .await
            }

            BookingEvent::Released { booking_id, reason } => {
                bookings
                    .transition(
                        booking_id,
                        &[booking_status::RESERVED],
                        BookingMirrorPatch::released(reason),
                    )
                    .await
            }

            BookingEvent::Cancelled { booking_id, reason } => {
                bookings
                    .transition(
                        booking_id,
                        &[booking_status::CONFIRMED],
                        BookingMirrorPatch::cancelled(reason),
                    )
                    .await
            }
        }?;

        shared::db::set_version(tx, "booking", &booking_id, version).await
    }
}

// `PaymentProjector` is gone. This service's own PAYMENTS events are no longer
// projected back in: `payment_service` and `settlement_worker_service` write the
// `payment` and `payout` rows directly, inside the transaction that enqueues the
// event. Consuming its own stream would have re-applied writes it had already
// made.
//
// What remains is the BOOKINGS mirror above — a *foreign* stream, which is exactly
// what a projector is still for.
