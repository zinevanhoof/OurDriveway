use bus::Projector;
use shared::{
    error::myerror::{MyError, MyResult},
    events::{Envelope, STREAM_SPOTS, spot::SpotEvent},
};

use crate::repository::spot_repository::SpotRepository;

/// Applies SPOTS events to this instance's local projection.
///
/// Deliberately thin: all the SurrealQL lives in the repository, and everything
/// non-deterministic (ids, timestamps) comes out of the event rather than being
/// computed here — otherwise two replicas replaying the same log would diverge.
pub struct SpotProjector {
    pub repository: SpotRepository,
}

impl Projector for SpotProjector {
    const STREAM: &'static str = STREAM_SPOTS;

    async fn last_seq(&self) -> MyResult<u64> {
        self.repository.last_seq().await
    }

    async fn apply(&self, payload: &[u8], seq: u64) -> MyResult<()> {
        let envelope: Envelope<SpotEvent> = serde_json::from_slice(payload)
            .map_err(|e| MyError::Bus(format!("decode SpotEvent at seq {seq}: {e}")))?;
        self.repository.apply(envelope, seq).await
    }
}
