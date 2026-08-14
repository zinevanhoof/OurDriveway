//! Turns a successful payment into a confirmed booking.
//!
//! A `Worker`, not a `Projector`, and the distinction is the whole reason this is a
//! separate file — see the table in bus/src/worker.rs. Publishing `Confirmed` is a side
//! effect: a projector would run it on every replica, and a fresh replica replaying
//! PAYMENTS from sequence 1 would re-confirm every booking ever paid for. A durable
//! consumer means one replica handles each payment, starting at the head.
//!
//! This service keeps no PAYMENTS projection and needs none — the event carries
//! everything except the booking's spot, which its own BOOKINGS projection already has.

use std::sync::Arc;

use bus::Worker;
use shared::{
    error::myerror::{MyError, MyResult},
    events::{Envelope, STREAM_PAYMENTS, payment::PaymentEvent},
};

use crate::service::booking_service::BookingService;

pub struct PaymentWorker {
    pub booking_service: Arc<BookingService>,
}

impl Worker for PaymentWorker {
    const STREAM: &'static str = STREAM_PAYMENTS;
    /// Shared by every replica — that shared name is the entire mechanism by which this
    /// becomes work-sharing rather than fan-out. Changing it creates a new consumer
    /// starting at the head, silently skipping every payment the old one had not yet
    /// confirmed.
    const DURABLE: &'static str = "booking-payments";

    async fn handle(&self, payload: &[u8], seq: u64) -> MyResult<()> {
        let envelope: Envelope<PaymentEvent> = serde_json::from_slice(payload)
            .map_err(|e| MyError::Bus(format!("decode PaymentEvent at seq {seq}: {e}")))?;

        let (booking_id, payment_id) = match envelope.payload {
            PaymentEvent::Succeeded {
                booking_id,
                payment_id,
                ..
            } => (booking_id, payment_id),

            // `Failed` is deliberately ignored rather than releasing the hold: the
            // renter can confirm the same intent again with another method, and if they
            // walk away the expiry sweeper collects it. Refunds and intent cancellation
            // are payment-service's business, not this stream's.
            _ => return Ok(()),
        };

        tracing::info!(%booking_id, %payment_id, "payment succeeded, confirming");

        // An error NAKs and comes back in thirty seconds, which is what covers the one
        // ordering that can go wrong here: a booking whose `Reserved` this instance's
        // BOOKINGS projector has not applied yet.
        self.booking_service
            .confirm_paid(booking_id, payment_id)
            .await?;

        Ok(())
    }
}
