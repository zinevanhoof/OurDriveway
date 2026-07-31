use bus::Projector;
use shared::{
    error::myerror::{MyError, MyResult},
    events::{
        Envelope, STREAM_SESSIONS, STREAM_USERS, session::SessionEvent, user::UserEvent,
    },
};

use crate::repository::{
    refresh_token_repository::RefreshTokenRepository, user_repository::UserRepository,
};

/// Applies USERS events to this instance's local projection — the only writer
/// to the `user` table. Handlers publish; they never write.
pub struct UserProjector {
    pub repository: UserRepository,
}

impl Projector for UserProjector {
    const STREAM: &'static str = STREAM_USERS;

    async fn last_seq(&self) -> MyResult<u64> {
        self.repository.last_seq().await
    }

    async fn apply(&self, payload: &[u8], seq: u64) -> MyResult<()> {
        let envelope: Envelope<UserEvent> = serde_json::from_slice(payload)
            .map_err(|e| MyError::Bus(format!("decode UserEvent at seq {seq}: {e}")))?;
        self.repository.apply(envelope, seq).await
    }
}

/// Applies SESSIONS events to the local `refresh_token` projection.
///
/// Separate consumer from USERS, so a login can be applied before the account it
/// belongs to in principle — harmless here, because a refresh token is looked up
/// by its own hash and never traverses to the user row.
pub struct SessionProjector {
    pub repository: RefreshTokenRepository,
}

impl Projector for SessionProjector {
    const STREAM: &'static str = STREAM_SESSIONS;

    async fn last_seq(&self) -> MyResult<u64> {
        self.repository.last_seq().await
    }

    async fn apply(&self, payload: &[u8], seq: u64) -> MyResult<()> {
        let envelope: Envelope<SessionEvent> = serde_json::from_slice(payload)
            .map_err(|e| MyError::Bus(format!("decode SessionEvent at seq {seq}: {e}")))?;
        self.repository.apply(envelope, seq).await
    }
}
