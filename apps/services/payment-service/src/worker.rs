//! The two side-effect consumers. Both funnel into
//! [`SettlementWorkerService::settle_up`].
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
use sqlx::PgPool;

use crate::service::settlement_worker_service::SettlementWorkerService;

/// How long to wait for this service's booking mirror to catch up to the event being
/// handled. Past this, fail and let the NAK bring the message back — a projector that
/// is more than a moment behind is a problem to be retried, not waited out inside a
/// consumer loop that is holding up every other message.
const PROJECTION_WAIT: Duration = Duration::from_secs(5);

/// Refunds and intent cancellations, triggered by a booking ending.
pub struct BookingWorker {
    pub service: Arc<SettlementWorkerService>,
    /// Read to check the booking mirror's version before deciding. Not written —
    /// that is `BookingProjector`'s job, on the same rows.
    pub db: PgPool,
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
        //
        // Waits on the **aggregate version**, not on a stream position. It used to be
        // `bus::await_applied(&self.applied, seq, …)`, reading the BOOKINGS
        // consumer's `ack_floor`; that stopped having a single value once the
        // projector became `PARTITIONS` consumers with independent cursors, and it
        // was always the coarser question — this booking reaching `version` is what
        // the decision actually needs, not everything published before it.
        //
        // Erroring rather than proceeding stale: we are about to decide whether to move
        // money, and the whole point of waiting is that the decision reads state
        // including the event that prompted it. The NAK is the retry.
        let version = envelope.version;
        if !bus::await_version::reached(&self.db, "booking", &booking_id, version, PROJECTION_WAIT)
            .await
        {
            return Err(MyError::Bus(format!(
                "{STREAM_BOOKINGS} mirror of booking:{booking_id} has not reached \
                 version {version} (seq {seq}); retrying"
            )));
        }

        self.service.settle_up(&booking_id).await
    }
}

/// The other edge: a payment resolving for a booking that has already ended.
///
/// Needed because the orderings are genuinely independent. A Bancontact payment can
/// land after the hold lapsed, in which case the BOOKINGS worker already ran and found
/// nothing but an unpaid intent.
pub struct PaymentWorker {
    pub service: Arc<SettlementWorkerService>,
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

        // No wait here, unlike `BookingWorker` above, and the asymmetry is the point:
        // PAYMENTS is this service's *own* stream. The payment row is written inside
        // the transaction that enqueues the event, so by the time this event exists
        // at all the row it describes is already committed — there is no projection
        // left to be behind.
        //
        // It used to hold a `watch::Receiver<u64>` for the PAYMENTS cursor and wait
        // on it. That projector was deleted when this service started writing its own
        // rows, and the receiver it asked `Readiness` for went with it — leaving an
        // `.expect` on a stream no longer registered, which panicked this service on
        // boot.
        self.service.settle_up(&booking_id).await
    }
}
