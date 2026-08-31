use uuid::Uuid;

use crate::events::user::{UserRegistered, UserUpdated};

/// The `app_user` table in the read model.
///
/// Note what is **absent: no password hash, ever.** Every row here is readable by
/// anyone — spot-owner profiles have to resolve for everyone — so the projection is
/// the first thing deciding what can possibly leak. `email` is the one sensitive
/// field, and it is cut per-caller in the response type rather than filtered per-row.
///
/// `email_verified` is absent for the same reason and is not an oversight: it is an
/// authentication concern that stays in user-service's private projection. Adding it
/// here would publish which addresses are unconfirmed to every client that can read
/// a spot owner's profile.
#[derive(Clone, Debug, sqlx::FromRow)]
pub struct ViewUser {
    pub id: Uuid,
    /// user-service's version of this user, as last applied here. What
    /// `bus::await_version` compares a client's `X-Await-Version` against.
    ///
    /// It stays a field on the model, though the reason has changed. It used to be
    /// load-bearing against a `CONTENT $row` write that replaced the whole record: a
    /// model missing this column *cleared* it, which on an existing row was a
    /// coercion failure that stalled the projector — and only ever on a re-applied
    /// `Registered`, so it took a rebuild to surface. Writes name their columns
    /// explicitly now, so an omission would be a compile error rather than a silent
    /// clear. The field remains because the version is genuinely read.
    #[sqlx(try_from = "i64")]
    pub version: u64,
    pub first_name: String,
    pub last_name: String,
    pub profile_picture: Option<String>,
    /// Not an `Option`, and `app_user.email` is `NOT NULL` — see
    /// `migrations/view/0002_user_email_not_null.sql`. Every row here is created by
    /// `UserRegistered`, which carries a `String`; `UserUpdated` only reaches
    /// `ViewUserRepository::patch`, which is an `UPDATE` and cannot create one.
    pub email: String,
    /// Deliberately public: a host has to be able to recognise the car that turns up
    /// on their driveway, so this is not scoped the way `email` is.
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

// No `bind` — see the note in `domain_models::user::user`. sqlx binds positionally,
// so the binds live beside the `$n` placeholders in `ViewUserRepository::patch`.

impl ViewUser {
    /// The row a `Registered` writes. Safe to `upsert`: this table has no column any
    /// other stream owns.
    pub fn registered(e: UserRegistered, version: u64) -> Self {
        Self {
            id: e.user_id,
            version,
            first_name: e.first_name,
            last_name: e.last_name,
            profile_picture: None,
            email: e.email,
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
    /// The literal is exhaustive on purpose — a new field breaks compilation, and
    /// this is the test that should be read before adding one. It asserts on the
    /// struct rather than on any generated SQL, which is the stronger place: nothing
    /// reaches the read model that is not a field here.
    #[test]
    fn no_credential_columns_reach_the_read_model() {
        let row = ViewUser {
            id: Uuid::nil(),
            version: 1,
            first_name: String::new(),
            last_name: String::new(),
            profile_picture: None,
            email: String::new(),
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
