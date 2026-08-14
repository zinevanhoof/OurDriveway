use std::sync::Arc;

use bus::Projector;
use shared::{
    error::myerror::{MyError, MyResult},
    events::{
        Envelope, STREAM_BOOKINGS, STREAM_PAYMENTS, booking::BookingEvent, payment::PaymentEvent,
    },
};

use crate::repository::payment_repository::PaymentRepository;

/// payment-service consumes BOOKINGS as well as its own stream, for two things it
/// cannot answer otherwise: what a booking costs and who it belongs to (before a
/// payment may be created), and whether it is still going to happen (before money is
/// kept or returned).
///
/// Read-only as far as the outside world is concerned. The refund side effect that
/// *reacts* to these same events is a `Worker`, not this projector — see worker.rs for
/// why that distinction is not optional.
pub struct BookingProjector {
    pub repository: Arc<PaymentRepository>,
}

impl Projector for BookingProjector {
    const STREAM: &'static str = STREAM_BOOKINGS;

    async fn last_seq(&self) -> MyResult<u64> {
        self.repository.last_seq(STREAM_BOOKINGS).await
    }

    async fn apply(&self, payload: &[u8], seq: u64) -> MyResult<()> {
        let envelope: Envelope<BookingEvent> = serde_json::from_slice(payload)
            .map_err(|e| MyError::Bus(format!("decode BookingEvent at seq {seq}: {e}")))?;

        self.repository.apply_booking(envelope, seq).await
    }
}

/// This service's own stream, projected back into the `payment` and `payout` tables.
///
/// Handlers publish and never write, so this is the only path by which a payment row
/// comes to exist — including the one the request that created it will read back.
pub struct PaymentProjector {
    pub repository: Arc<PaymentRepository>,
}

impl Projector for PaymentProjector {
    const STREAM: &'static str = STREAM_PAYMENTS;

    async fn last_seq(&self) -> MyResult<u64> {
        self.repository.last_seq(STREAM_PAYMENTS).await
    }

    async fn apply(&self, payload: &[u8], seq: u64) -> MyResult<()> {
        let envelope: Envelope<PaymentEvent> = serde_json::from_slice(payload)
            .map_err(|e| MyError::Bus(format!("decode PaymentEvent at seq {seq}: {e}")))?;

        self.repository.apply_payment(envelope, seq).await
    }
}
