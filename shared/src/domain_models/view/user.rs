use surrealdb::types::SurrealValue;
use uuid::Uuid;

use surrealdb::types::vars;
use surrealdb::{engine::remote::ws::Client, method::Query};

use crate::events::user::{UserRegistered, UserUpdated};

/// The `user` table in the read model.
///
/// Note what is **absent: no password hash, ever.** This table is world-readable
/// (`FOR select WHERE true`), so the projection is the first thing deciding what can
/// possibly leak. `email` is the one sensitive field here and it is guarded at the
/// *field* level in view-schema.surql — a row stays selectable by anyone, the
/// address does not.
///
/// `email_verified` is absent for the same reason and is not an oversight: it is an
/// authentication concern that stays in user-service's private projection. Adding it
/// here would publish which addresses are unconfirmed to every client that can read
/// a spot owner's profile.
#[derive(Clone, Debug, SurrealValue)]
pub struct ViewUser {
    pub id: Uuid,
    /// user-service's version of this user, as last applied here. What
    /// `bus::await_version` compares a client's `X-Await-Version` against.
    ///
    /// **A field on the model, not only a column**, and the difference is a bug that
    /// took a rebuild to surface. This table is written with `CONTENT $row`, which
    /// replaces the whole record — so a model without this column *clears* it. On a
    /// row that does not exist yet the schema's `DEFAULT 0` fills it back in and
    /// nothing looks wrong; on one that does, the write is a coercion failure
    /// (`Expected int but found NONE`) that stalls the projector.
    ///
    /// Which means it only ever fired on a **re-applied** `Registered`: a backfill,
    /// or a redelivery outside the stream's `duplicate_window`. Carrying the version
    /// in the row is what makes the whole-row write total.
    pub version: u64,
    pub first_name: String,
    pub last_name: String,
    pub profile_picture: Option<String>,
    pub email: Option<String>,
    /// Deliberately public: a host has to be able to recognise the car that turns up
    /// on their driveway, so this is not scoped the way `email` is.
    ///
    /// `license_plates ?? []` when selected, for rows written before the column
    /// existed.
    pub license_plates: Vec<String>,
}

/// A partial update to a [`ViewUser`], written with the struct-update idiom:
///
/// ```ignore
/// ViewUserPatch { email: Some(addr), ..Default::default() }
/// ```
///
/// Five columns, which is every column this table has apart from the key. Nothing
/// a USERS event carries is withheld here — the withholding happened in [`ViewUser`]
/// itself, which has no password and no `email_verified`.
#[derive(Debug, Default)]
pub struct ViewUserPatch {
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub profile_picture: Option<String>,
    pub email: Option<String>,
    pub license_plates: Option<Vec<String>>,
}

impl ViewUserPatch {
    /// Binds every patchable column. Absent ones bind as NONE, which the
    /// `?? column` in `ViewUserRepository::patch` turns into "leave it alone".
    pub fn bind(self, q: Query<'_, Client>) -> Query<'_, Client> {
        q.bind(vars! {
            first_name:      self.first_name,
            last_name:       self.last_name,
            profile_picture: self.profile_picture,
            email:           self.email,
            license_plates:  self.license_plates,
        })
    }
}

impl ViewUser {
    /// The row a `Registered` writes. Safe to `upsert`: this table has no link
    /// column and no column any other stream owns.
    pub fn registered(e: UserRegistered, version: u64) -> Self {
        Self {
            id: e.user_id,
            version,
            first_name: e.first_name,
            last_name: e.last_name,
            profile_picture: None,
            email: Some(e.email),
            license_plates: Vec::new(),
        }
    }
}

impl From<UserUpdated> for ViewUserPatch {
    fn from(e: UserUpdated) -> Self {
        Self {
            first_name: e.first_name,
            last_name: e.last_name,
            profile_picture: e.profile_picture,
            email: e.email,
            license_plates: e.license_plates,
            ..Self::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_covers_every_patchable_column() {
        let _: ViewUserPatch = ViewUserPatch {
            first_name: None,
            last_name: None,
            profile_picture: None,
            email: None,
            license_plates: None,
        };
    }

    /// The whole reason this model exists separately from
    /// `domain_models::user::User`. A password column here would be world-readable.
    ///
    /// This used to assert on the derived `COLUMNS` string. It now asserts on the
    /// struct, which is the stronger place: `ViewUserRepository::upsert` writes this
    /// row with `CONTENT $row`, so the struct's fields **are** the columns written.
    /// The literal is exhaustive on purpose — a new field breaks compilation, and
    /// this is the test that should be read before adding one.
    #[test]
    fn no_credential_columns_reach_the_read_model() {
        let row = ViewUser {
            id: Uuid::nil(),
            version: 1,
            first_name: String::new(),
            last_name: String::new(),
            profile_picture: None,
            email: None,
            license_plates: Vec::new(),
        };
        let written = format!("{row:?}");
        for forbidden in ["password", "email_verified"] {
            assert!(
                !written.contains(forbidden),
                "{forbidden} must never be projected into the world-readable view"
            );
        }
    }
}
