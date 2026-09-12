use diesel::prelude::*;
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
/// `Queryable + Selectable`, which is the row and nothing else.
///
/// `Selectable` rather than bare `Queryable` so a read is
/// `.select(User::as_select())` — that binds the struct to the schema by NAME and
/// checks it at compile time, where `SELECT *` matched by position and would silently
/// mis-decode if two same-typed columns were ever reordered in a migration.
#[derive(Clone, Debug, Queryable, Selectable, Insertable, AsChangeset)]
#[diesel(table_name = crate::schema::user::app_user)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct User {
    /// A plain uuid primary key. It used to be the record key `user:⟨uuid⟩`, which
    /// is why every SELECT carried `record::id(id) AS id` — there is nothing to
    /// unwrap now, so `SELECT *` is enough.
    pub id: Uuid,
    /// Bumped by user-service inside the transaction that writes this row. The
    /// version a client waits on, and the gap detector for an out-of-order event —
    /// see `shared::events::Envelope`.
    ///
    /// It is no longer "the key concurrent writers collide on": under Read Committed
    /// two writers to one row do not conflict, so `db::next_version` takes a
    /// `FOR UPDATE` on the row instead. See the note there.
    ///
    /// `i64` and not `u64`, so the column's type is the field's type and nothing
    /// converts. This was a `u64` behind a `diesel_ext::Version` newtype for as long as
    /// `format_version` and `Envelope::version` were unsigned; what that bought was
    /// "a negative version is not representable", against a value only someone in psql
    /// can write — while three `sql_query` reads clamped by hand anyway, because
    /// `QueryableByName` never sees `deserialize_as`. The one place unsignedness earned
    /// its keep is `parse_version`, which reads a client-supplied header and still
    /// refuses a negative there.
    pub version: i64,
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
    /// ISO 3166-1 alpha-2, uppercase. `None` until the profile is filled in, which
    /// is most accounts: it is only needed to open a Stripe connected account, and
    /// only hosts ever do that.
    pub country: Option<String>,
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
    pub fn registered(e: UserRegistered, version: i64) -> Self {
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
            country: None,
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
///
/// `AsChangeset` gives the absent-is-unchanged semantics the hand-written
/// `COALESCE($n, column)` used to: a `None` field is omitted from the `SET` list
/// entirely. Same result, and the fields are matched by name rather than by position —
/// which is what removes the swapped-bind hazard the repository used to warn about.
#[derive(Debug, Default, AsChangeset)]
#[diesel(table_name = crate::schema::user::app_user)]
pub struct UserPatch {
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub email: Option<String>,
    pub password: Option<String>,
    pub profile_picture: Option<String>,
    pub license_plates: Option<Vec<String>>,
    pub email_verified: Option<bool>,
    pub country: Option<String>,
}

// There is no `bind` here, and the reason changed twice. It existed because SurrealDB
// binds by NAME, so a patch could hand a query its whole field set in one call. Under
// sqlx it went away because binds are POSITIONAL and had to sit beside the `$n`
// placeholders in the repository — splitting them across two files was a silent
// reordering hazard.
//
// Under diesel there is nothing to split: `AsChangeset` derives the `SET` list from
// this struct's own fields, absent-is-unchanged included, so the patch and the
// statement cannot disagree about an order because there is no order.
//
// `set_covers_every_patchable_column` below is what still couples this struct to the
// columns it is allowed to write.

impl From<UserUpdated> for UserPatch {
    fn from(e: UserUpdated) -> Self {
        Self {
            first_name: e.first_name,
            last_name: e.last_name,
            email: e.email,
            profile_picture: e.profile_picture,
            license_plates: e.license_plates,
            country: e.country,
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
            country: None,
        };
    }
}
