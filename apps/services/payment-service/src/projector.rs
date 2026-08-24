use bus::Projector;
use chrono::{DateTime, Utc};
use shared::{
    domain_models::{
        booking::status as booking_status,
        payment::{BookingMirror, BookingMirrorPatch, Payment, PaymentPatch, Payout, status},
    },
    error::myerror::MyResult,
    events::{STREAM_BOOKINGS, STREAM_PAYMENTS, booking::BookingEvent, payment::PaymentEvent},
};
use surrealdb::{engine::remote::ws::Client, method::Transaction};

use crate::repository::{
    booking_mirror_repository::BookingMirrorRepository, payment_repository::PaymentRepository,
    payout_repository::PayoutRepository,
};

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
    type Event = BookingEvent;

    async fn apply(
        &self,
        tx: &Transaction<Client>,
        event: BookingEvent,
        _at: DateTime<Utc>,
        _seq: u64,
    ) -> MyResult<()> {
        let bookings = BookingMirrorRepository { q: tx };

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
        }
    }
}

/// This service's own stream, projected back into the `payment` and `payout` tables.
///
/// Handlers publish and never write, so this is the only path by which a payment row
/// comes to exist — including the one the request that created it will read back.
pub struct PaymentProjector;

impl Projector for PaymentProjector {
    const STREAM: &'static str = STREAM_PAYMENTS;
    type Event = PaymentEvent;

    async fn apply(
        &self,
        tx: &Transaction<Client>,
        event: PaymentEvent,
        _at: DateTime<Utc>,
        _seq: u64,
    ) -> MyResult<()> {
        let payments = PaymentRepository { q: tx };

        match event {
            PaymentEvent::Created(e) => payments.upsert(Payment::created(e)).await,

            // Reachable from `created` *or* `failed`: a renter whose first attempt was
            // declined retries on the same session, and refusing that transition would
            // leave a paid booking stuck as failed.
            PaymentEvent::Succeeded {
                payment_id,
                intent_id,
                ..
            } => {
                payments
                    .transition(
                        payment_id,
                        &status::UNPAID,
                        PaymentPatch::succeeded(intent_id),
                    )
                    .await
            }

            // From itself as well as from `created`: two declines in a row are two
            // genuine failures and the second must still record its reason.
            PaymentEvent::Failed {
                payment_id, reason, ..
            } => {
                payments
                    .transition(payment_id, &status::UNPAID, PaymentPatch::failed(reason))
                    .await
            }

            PaymentEvent::Refunded {
                payment_id,
                refund_id,
                ..
            } => {
                payments
                    .transition(
                        payment_id,
                        &[status::SUCCEEDED],
                        PaymentPatch::refunded(refund_id),
                    )
                    .await
            }

            // An unpaid session, which is `created` *or* `failed` — a booking whose
            // renter was declined and then walked away still has a live session that
            // has to be voided.
            PaymentEvent::SessionExpired { payment_id, .. } => {
                payments
                    .transition(payment_id, &status::UNPAID, PaymentPatch::expired())
                    .await
            }

            // A different table entirely, and the one event here that is not about a
            // payment. `Payout::requested` is what turns the event into a row.
            ref e @ PaymentEvent::PayoutRequested { .. } => {
                let Some(payout) = Payout::requested(e) else {
                    return Ok(());
                };
                PayoutRepository { q: tx }.upsert(payout).await
            }
        }
    }
}
