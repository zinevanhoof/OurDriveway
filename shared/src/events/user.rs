use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The `$token.ID` string for a user: `"user:019fafc9…"` (hyphen-free).
///
/// Built explicitly rather than via `RecordId::to_sql()`, so the format is
/// decided in exactly one place and can't drift from the strings stored in
/// `owner_id`/`renter_id` elsewhere.
pub fn user_claim_id(user_id: &Uuid) -> String {
    format!("user:{}", record_key(user_id))
}

/// A uuid as a SurrealDB record key.
///
/// `simple()`, not the hyphenated form: hyphens aren't valid in a bare
/// identifier, so SurrealDB stores such a key quoted (``user:`019f-…` ``) while
/// GraphQL emits it unquoted. The value then fails to resolve when a client
/// passes it back to `user(id: …)` or `spot(id: …)` — silently returning null.
pub fn record_key(id: &Uuid) -> String {
    id.simple().to_string()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum UserEvent {
    Registered(UserRegistered),
    Updated(UserUpdated),
    PasswordChanged(UserPasswordChanged),
    /// The address was proven reachable — somebody opened a link only that
    /// mailbox received. Published by user-service after checking the token.
    EmailVerified { user_id: Uuid },
    /// "Send that link again." Raised by user-service when a user asks for a new
    /// verification email, and consumed only by notification-service — nothing
    /// projects it.
    ///
    /// Carries the address and name rather than just an id so notification-service
    /// never has to look a user up, which is what lets it own no database at all.
    /// `PasswordResetRequested` will be this same shape.
    VerificationRequested(VerificationRequested),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VerificationRequested {
    pub user_id: Uuid,
    pub email: String,
    pub first_name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UserRegistered {
    pub user_id: Uuid,
    pub shard: String,
    pub first_name: String,
    pub last_name: String,
    pub email: String,
    /// Argon2 PHC string, hashed in Rust before publishing.
    ///
    /// Never the plaintext, and never hashed in a projection: Argon2 salts
    /// randomly, so every replica would compute a different hash for the same
    /// event and logins would work on one instance but not another.
    pub password_hash: String,
}

/// Partial update — `None` means "unchanged", not "clear".
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UserUpdated {
    pub user_id: Uuid,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub profile_picture: Option<String>,
    pub email: Option<String>,
    /// The whole list, or `None`. There is no add/remove event: the edit form
    /// submits every plate it knows about, so a diff would mean the client
    /// deciding what "unchanged" is.
    pub license_plates: Option<Vec<String>>,
}

/// Its own event rather than a field on [`UserUpdated`]: that one is consumed by
/// the view projection, and a password hash must never reach a database a
/// browser identity can read.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UserPasswordChanged {
    pub user_id: Uuid,
    /// Argon2 PHC string. Hashed on the write side for the same reason as
    /// [`UserRegistered::password_hash`].
    pub password_hash: String,
}
