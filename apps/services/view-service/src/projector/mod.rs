use std::sync::Arc;

use bus::Projector;
use shared::{
    error::myerror::{MyError, MyResult},
    events::{Envelope, STREAM_SPOTS, STREAM_USERS, spot::SpotEvent, user::UserEvent},
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
