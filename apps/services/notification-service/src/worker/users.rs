//! Decodes USERS and hands each event to [`UserWorkerService`]. Nothing decides
//! anything here — see `service/user_worker_service.rs` for what an event becomes.

use std::sync::Arc;

use bus::Worker;
use shared::{
    error::myerror::{MyError, MyResult},
    events::{Envelope, STREAM_USERS, user::UserEvent},
};

use crate::service::user_worker_service::UserWorkerService;

/// Turns USERS events into email.
pub struct UserWorker {
    pub service: Arc<UserWorkerService>,
}

impl Worker for UserWorker {
    const STREAM: &'static str = STREAM_USERS;

    /// Shared by every replica. Renaming this creates a *fresh* consumer starting
    /// at `New`, silently dropping anything the old one had not delivered yet.
    const DURABLE: &'static str = "notification-users";

    async fn handle(&self, payload: &[u8], seq: u64) -> MyResult<()> {
        let envelope: Envelope<UserEvent> = serde_json::from_slice(payload)
            .map_err(|e| MyError::Bus(format!("decode UserEvent at seq {seq}: {e}")))?;

        self.service.notify(&envelope).await
    }
}
