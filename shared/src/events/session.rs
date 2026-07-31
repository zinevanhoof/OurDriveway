use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Refresh-token lifecycle.
///
/// Only ever the **hash** of a token, never the token itself: the plaintext is a
/// bearer credential, and an event log is permanent and replicated to every
/// instance. Hashing happens on the write side (SHA-256, so unlike Argon2 it
/// *would* be deterministic in a projection — but putting the plaintext on the
/// wire to achieve that is the wrong trade).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SessionEvent {
    Issued(RefreshTokenIssued),
    /// Revoke-old-and-issue-new as **one** event.
    ///
    /// Two separate appends would leave a window where the old token is dead and
    /// the new one doesn't exist yet. That window fails closed (the user simply
    /// logs in again), but one event is atomic by construction and models what
    /// actually happened.
    Rotated(RefreshTokenRotated),
    Revoked(RefreshTokenRevoked),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RefreshTokenIssued {
    pub token_id: Uuid,
    pub user_id: Uuid,
    /// The *user's* shard — sessions live on the user's subject so a user's whole
    /// session history stays on one ordered subject.
    pub shard: String,
    pub token_hash: String,
    pub jti: Uuid,
    pub expires_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RefreshTokenRotated {
    pub old_token_hash: String,
    pub token_id: Uuid,
    pub user_id: Uuid,
    pub shard: String,
    pub token_hash: String,
    pub jti: Uuid,
    pub expires_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RefreshTokenRevoked {
    pub token_hash: String,
    pub reason: String,
}
