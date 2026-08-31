use uuid::Uuid;

use crate::events::user::{UserRegistered, UserUpdated};

/// The `app_user` table, whole. One definition for every service that reads it.
///
/// The table is `app_user` and the aggregate is `user`; see `shared::db::table_for`
/// for why (`user` is reserved, and an unquoted `FROM user` silently reads the
/// current-user keyword instead of erroring).
///
/// Read entire rather than per-use-case: this replaced a `UserAuth` that selected
/// five columns for login and a separate query for the greeting, which is two
/// round trips and two shapes to keep in step for the sake of four small columns.
///
/// Carries no methods. Reading and writing it is `UserRepository`'s job.
#[derive(Clone, Debug, sqlx::FromRow)]
pub struct User {
    /// A plain uuid primary key. It used to be the record key `user:⟨uuid⟩`, which
    /// is why every SELECT carried `record::id(id) AS id` — there is nothing to
    /// unwrap now, so `SELECT *` is enough.
    pub id: Uuid,
    /// Bumped by user-service inside the transaction that writes this row. The
    /// token a client waits on, and the gap detector for an out-of-order event —
    /// see `shared::events::Envelope`.
    ///
    /// It is no longer "the key concurrent writers collide on": under Read Committed
    /// two writers to one row do not conflict, so `db::next_version` takes a
    /// `FOR UPDATE` on the row instead. See the note there.
    ///
    /// `try_from` because the column is `bigint` (i64) and this is a `u64` —
    /// `format_version` and `Envelope::version` are unsigned all the way through, and
    /// a negative version is not representable rather than merely unexpected.
    #[sqlx(try_from = "i64")]
    pub version: u64,
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
    /// `text[]`, mapped natively. The `license_plates ?? []` that used to be in
    /// every SELECT is gone: the column is `NOT NULL DEFAULT '{}'`, so there is no
    /// absent case left to paper over.
    pub license_plates: Vec<String>,
    /// Gates login. On the same row as the password hash deliberately — one
    /// lookup, and no way to check the credential without also holding the flag.
    pub email_verified: bool,
}

impl User {
    /// The row a `Registered` writes.
    ///
    /// Was `From<UserRegistered>` until the row carried a version — that is assigned
    /// by the transaction, not by the event, so there is a second argument now and
    /// no total conversion left to implement. Same shape as [`super::super::spot::Spot::created`]
    /// and `Booking::created`.
    ///
    /// The three columns the event does not carry are defaults of the model; they
    /// used to be literals in the projector's `CONTENT` block.
    pub fn registered(e: UserRegistered, version: u64) -> Self {
        Self {
            id: e.user_id,
            version,
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
/// Only the columns an edit can touch.
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

// There is no `bind` here any more. It existed because SurrealDB binds by NAME, so a
// patch could hand a query its whole field set in one call and the `SET` list picked
// them up by name. sqlx binds POSITIONALLY, so the binds have to sit in the same order
// as the `$n` placeholders — which is in the statement, and therefore in the
// repository. Splitting them across two files would be a silent reordering hazard: the
// columns are all `Option<T>` and several are the same type, so a swapped pair would
// compile and write the wrong column.
//
// `set_covers_every_patchable_column` below is what still couples the two.

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
    /// the `SET` list in `UserRepository::patch` needs it too, *and* a `.bind()` in
    /// the matching position. That coupling used to be held by a `bind` method here;
    /// positional binds moved it into the repository, so this test is now the only
    /// thing pointing at it.
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
