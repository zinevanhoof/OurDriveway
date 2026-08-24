use surrealdb::types::SurrealValue;
use surrealdb::types::vars;
use surrealdb::{engine::remote::ws::Client, method::Query};
use uuid::Uuid;

use crate::events::user::{UserRegistered, UserUpdated};

/// The `user` table, whole. One definition for every service that reads it.
///
/// Read entire rather than per-use-case: this replaced a `UserAuth` that selected
/// five columns for login and a separate query for the greeting, which is two
/// round trips and two shapes to keep in step for the sake of four small columns.
///
/// Carries no methods. Reading and writing it is `UserRepository`'s job.
#[derive(Clone, Debug, SurrealValue)]
pub struct User {
    /// Stored as `user:⟨uuid⟩`. Every SELECT says `record::id(id) AS id`, which is
    /// what lets this be a plain uuid — the JWT carries one, and nothing outside
    /// the database should have to know about record keys.
    pub id: Uuid,
    /// Read, never recomputed. `shard_of` would agree today, but this selects the
    /// subject the user's whole history lives on, so a changed SHARD_COUNT would
    /// send their next event where no reader is looking.
    pub shard: String,
    pub first_name: String,
    pub last_name: String,
    /// `email_idx … UNIQUE` in `schemas/user-schema.surql`. That index is the real
    /// guard against a duplicate signup; the lookup only turns the common case
    /// into a 409 instead of a projector failure.
    pub email: String,
    /// Argon2 PHC string. Verified in Rust (`auth::password`), never by SurrealQL:
    /// hashing has to happen on the write side for determinism, so verification
    /// lives next to it.
    pub password: String,
    pub profile_picture: Option<String>,
    /// Rows written before the field existed hold NONE, which will not
    /// deserialize into a Vec — hence `license_plates ?? []` in every statement
    /// that selects this table.
    pub license_plates: Vec<String>,
    /// Gates login. On the same row as the password hash deliberately — one
    /// lookup, and no way to check the credential without also holding the flag.
    ///
    /// `email_verified ?? false` when selected, for rows written before it existed.
    pub email_verified: bool,
}

impl From<UserRegistered> for User {
    /// The three columns the event does not carry. These used to be literals in
    /// the projector's `CONTENT` block; they are defaults of the model, so they
    /// belong where the model is.
    fn from(e: UserRegistered) -> Self {
        Self {
            id: e.user_id,
            shard: e.shard,
            first_name: e.first_name,
            last_name: e.last_name,
            email: e.email,
            password: e.password_hash,
            profile_picture: None,
            license_plates: Vec::new(),
            email_verified: false,
        }
    }
}

/// A partial update to a [`User`], written with the struct-update idiom:
///
/// ```ignore
/// UserPatch { password: Some(hash), ..Default::default() }
/// ```
///
/// `shard` is absent on purpose: it selects the subject a user's whole history is
/// ordered on, so it is written once at registration and never edited.
#[derive(Debug, Default)]
pub struct UserPatch {
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub email: Option<String>,
    pub password: Option<String>,
    pub profile_picture: Option<String>,
    pub license_plates: Option<Vec<String>>,
    pub email_verified: Option<bool>,
}

impl UserPatch {
    /// Binds every patchable column. Absent ones bind as NONE, which the
    /// `?? column` in `UserRepository::patch` turns into "leave it alone".
    ///
    /// Every field here has to appear in that statement's `SET` list, and vice
    /// versa.
    pub fn bind(self, q: Query<'_, Client>) -> Query<'_, Client> {
        q.bind(vars! {
            first_name:      self.first_name,
            last_name:       self.last_name,
            email:           self.email,
            password:        self.password,
            profile_picture: self.profile_picture,
            license_plates:  self.license_plates,
            email_verified:  self.email_verified,
        })
    }
}

impl From<UserUpdated> for UserPatch {
    fn from(e: UserUpdated) -> Self {
        Self {
            first_name: e.first_name,
            last_name: e.last_name,
            email: e.email,
            profile_picture: e.profile_picture,
            license_plates: e.license_plates,
            ..Self::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The SQL lives in `UserRepository` now, so nothing here can assert on it.
    /// What this *can* do is fail the moment the struct grows a field.
    ///
    /// The literal is **exhaustive on purpose** — no `..Default::default()`. Add a
    /// field to [`UserPatch`] and this stops compiling, which is the reminder that
    /// `bind` and the `SET` list in `patch` need it too.
    #[test]
    fn set_covers_every_patchable_column() {
        let _: UserPatch = UserPatch {
            first_name: None,
            last_name: None,
            email: None,
            password: None,
            profile_picture: None,
            license_plates: None,
            email_verified: None,
        };
    }
}
