//! Everything `worker::PaymentWorker` does once a payment settles.
//!
//! Named for its caller, and its own type, because that caller is a `Worker` and never
//! a route — the two paths obey opposite rules. `BookingService` authorizes a caller,
//! re-checks availability and publishes under compare-and-swap. None of that applies
//! once money has moved, and a method sitting among those that do is one refactor away
//! from acquiring them.
//!
//! Nothing here takes a caller id, because there is no caller — Stripe acted.

use std::sync::Arc;

use async_nats::jetstream::Context;
use shared::{
    error::myerror::{ContextExt, MyResult},
    events::{Envelope, booking::BookingEvent, booking_subject},
};
use surrealdb::{Surreal, engine::remote::ws::Client};
use uuid::Uuid;

use crate::policy::access::NOT_FOUND;
use crate::repository::booking_repository::BookingRepository;

/// Publishes `Confirmed` off a settled payment. Reads the booking only to address
/// its subject — `PaymentEvent::Succeeded` does not carry the spot.
pub struct PaymentWorkerService {
    pub js: Context,
    pub bookings: BookingRepository,
}

impl PaymentWorkerService {
    pub fn new(js: Context, db: Arc<Surreal<Client>>) -> Self {
        Self {
            js,
            bookings: BookingRepository { q: db },
        }
    }

    /// Payment succeeded, as reported by a signature-verified Stripe webhook.
    ///
    /// Called only by `worker::PaymentWorker`, never from a request. There is
    /// deliberately no renter-facing confirm endpoint: a renter who could confirm their
    /// own booking would not have to pay for it.
    ///
    /// This is *post*-capture, which is what makes it the opposite of
    /// `BookingService::release` in the one respect that matters — no
    /// `policy::access::authorize`, because there is no caller to authorize:
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
            .bookings
            .find_by_id(booking_id)
            .await?
            .context_not_found(NOT_FOUND)?;

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
}
