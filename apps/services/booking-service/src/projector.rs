use std::sync::Arc;

use bus::Projector;
use shared::{
    error::myerror::{MyError, MyResult},
    events::{
        Envelope, STREAM_BOOKINGS, STREAM_SPOTS, booking::BookingEvent, spot::SpotEvent,
    },
};

use crate::repository::booking_repository::BookingRepository;

/// booking-service consumes SPOTS as well as its own stream: it needs price,
/// availability and active-ness to authorize and price a booking server-side, and
/// the spot table lives in another service's database.
///
/// The two advance independently, which is exactly why a BOOKINGS event can arrive
/// for a spot this projector hasn't created yet — see the `option<>` fields in
/// booking-schema.surql.
pub struct SpotProjector {
    pub repository: Arc<BookingRepository>,
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
    pub repository: Arc<BookingRepository>,
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
