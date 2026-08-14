//! The two side-effect consumers. Both funnel into [`Settler::settle_up`].
//!
//! `Worker` and not `Projector`, and the difference is the whole reason this file
//! exists — see the table in bus/src/worker.rs. A projector is fan-out: every replica
//! applies every event. Refunding from a projector would issue one refund per running
//! instance, and a fresh replica replaying the log from sequence 1 would re-refund
//! every booking ever cancelled. A worker is a durable shared consumer: one replica
//! handles each message, it starts at the head rather than replaying history, and a
//! failure NAKs and comes back instead of stopping the process.

use std::{sync::Arc, time::Duration};

use bus::Worker;
use shared::{
    error::myerror::{MyError, MyResult},
    events::{
        Envelope, STREAM_BOOKINGS, STREAM_PAYMENTS, booking::BookingEvent, payment::PaymentEvent,
    },
};
use tokio::sync::watch;

use crate::service::settle::Settler;

/// How long to wait for this instance's own projector to catch up to the event being
/// handled. Past this, fail and let the NAK bring the message back — a projector that
/// is more than a moment behind is a problem to be retried, not waited out inside a
/// consumer loop that is holding up every other message.
const PROJECTION_WAIT: Duration = Duration::from_secs(5);

/// Refunds and intent cancellations, triggered by a booking ending.
pub struct BookingWorker {
    pub settler: Arc<Settler>,
    /// This instance's BOOKINGS projection cursor.
    pub applied: watch::Receiver<u64>,
}

impl Worker for BookingWorker {
    const STREAM: &'static str = STREAM_BOOKINGS;
    /// Shared by every replica. Changing this string creates a *new* consumer starting
    /// at the head, silently skipping every refund the old one had not yet delivered.
    const DURABLE: &'static str = "payment-bookings";

    async fn handle(&self, payload: &[u8], seq: u64) -> MyResult<()> {
        let envelope: Envelope<BookingEvent> = serde_json::from_slice(payload)
            .map_err(|e| MyError::Bus(format!("decode BookingEvent at seq {seq}: {e}")))?;

        // Only the two events that end a booking. Reserved and Confirmed change
        // nothing about money that is already where it should be.
        let booking_id = match envelope.payload {
            BookingEvent::Released { booking_id, .. }
            | BookingEvent::Cancelled { booking_id, .. } => booking_id,
            _ => return Ok(()),
        };

        // Wait for our own projection to include *this* event before deciding, or
        // `decide` would read the booking as still reserved and do nothing — and
        // nothing would ever trigger it again.
        wait_for(&self.applied, seq, STREAM_BOOKINGS).await?;

        self.settler.settle_up(&booking_id).await
    }
}

/// The other edge: a payment resolving for a booking that has already ended.
///
/// Needed because the orderings are genuinely independent. A Bancontact payment can
/// land after the hold lapsed, in which case the BOOKINGS worker already ran and found
/// nothing but an unpaid intent.
pub struct PaymentWorker {
    pub settler: Arc<Settler>,
    /// This instance's PAYMENTS projection cursor.
    pub applied: watch::Receiver<u64>,
}

impl Worker for PaymentWorker {
    const STREAM: &'static str = STREAM_PAYMENTS;
    const DURABLE: &'static str = "payment-payments";

    async fn handle(&self, payload: &[u8], seq: u64) -> MyResult<()> {
        let envelope: Envelope<PaymentEvent> = serde_json::from_slice(payload)
            .map_err(|e| MyError::Bus(format!("decode PaymentEvent at seq {seq}: {e}")))?;

        // Only a successful payment can require settling from this side. Refunded and
        // IntentCancelled are the *outcomes* of settling — reacting to them would loop.
        let booking_id = match envelope.payload {
            PaymentEvent::Succeeded { booking_id, .. } => booking_id,
            _ => return Ok(()),
        };

        wait_for(&self.applied, seq, STREAM_PAYMENTS).await?;

        self.settler.settle_up(&booking_id).await
    }
}

/// Blocks until this instance's projector for `stream` has applied `seq`.
///
/// `Err` on timeout rather than proceeding with stale data — the caller is about to
/// decide whether to move money, and the whole point of waiting is that the decision
/// reads state including the event that prompted it. Failing hands the message back to
/// NATS, which is the retry.
async fn wait_for(applied: &watch::Receiver<u64>, seq: u64, stream: &str) -> MyResult<()> {
    let mut rx = applied.clone();
    let caught_up = tokio::time::timeout(PROJECTION_WAIT, async {
        while *rx.borrow() < seq {
            if rx.changed().await.is_err() {
                // The projector dropped its sender, i.e. it stopped. Readiness has
                // already flipped; there is nothing to wait for.
                return false;
            }
        }
        true
    })
    .await;

    match caught_up {
        Ok(true) => Ok(()),
        _ => Err(MyError::Bus(format!(
            "{stream} projection has not reached seq {seq}; retrying"
        ))),
    }
}
