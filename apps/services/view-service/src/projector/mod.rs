use std::sync::Arc;

use bus::Projector;
use shared::{
    error::myerror::{MyError, MyResult},
    events::{
        Envelope, STREAM_BOOKINGS, STREAM_PAYMENTS, STREAM_SPOTS, STREAM_USERS,
        booking::BookingEvent, payment::PaymentEvent, spot::SpotEvent, user::UserEvent,
    },
};

use crate::repository::ViewRepository;

/// One projector per stream, each with its own consumer. They advance
/// independently, which is exactly why a spot can be applied before the user it
/// references — see the backfill in `ViewRepository::user_registered`.
pub struct UserProjector {
    pub repository: Arc<ViewRepository>,
}

impl Projector for UserProjector {
    const STREAM: &'static str = STREAM_USERS;

    async fn last_seq(&self) -> MyResult<u64> {
        self.repository.last_seq(STREAM_USERS).await
    }

    async fn apply(&self, payload: &[u8], seq: u64) -> MyResult<()> {
        let envelope: Envelope<UserEvent> = serde_json::from_slice(payload)
            .map_err(|e| MyError::Bus(format!("decode UserEvent at seq {seq}: {e}")))?;
        self.repository.apply_user(envelope, seq).await
    }
}

pub struct SpotProjector {
    pub repository: Arc<ViewRepository>,
}

impl Projector for SpotProjector {
    const STREAM: &'static str = STREAM_SPOTS;

    async fn last_seq(&self) -> MyResult<u64> {
        self.repository.last_seq(STREAM_SPOTS).await
    }

    async fn apply(&self, payload: &[u8], seq: u64) -> MyResult<()> {
        let envelope: Envelope<SpotEvent> = serde_json::from_slice(payload)
            .map_err(|e| MyError::Bus(format!("decode SpotEvent at seq {seq}: {e}")))?;
        self.repository.apply_spot(envelope, seq).await
    }
}

pub struct BookingProjector {
    pub repository: Arc<ViewRepository>,
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

/// PAYMENTS, for **payout history only**.
///
/// Deliberately not the payments themselves. A renter's charges and a host's income
/// figures are served by payment-service, which owns them — projecting them here too
/// would give the payout button one answer and the balance next to it another. What
/// belongs in the read model is the *list* of withdrawals, so it can be queried
/// alongside the rest of a profile like everything else.
pub struct PaymentProjector {
    pub repository: Arc<ViewRepository>,
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
