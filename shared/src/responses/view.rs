use serde::Serialize;
use uuid::Uuid;

use crate::domain_models::view::user::ViewUser;

/// The public half of a user's own row.
///
/// Deliberately narrower than [`ViewUser`]: `email` and `license_plates` are on the
/// model because the GraphQL layer serves them under their own permissions, but this
/// endpoint answers the header — a name and a picture — and has no reason to carry
/// anything else.
#[derive(Serialize)]
pub struct Profile {
    pub first_name: String,
    pub last_name: String,
    pub profile_picture: Option<String>,
}

impl From<ViewUser> for Profile {
    fn from(u: ViewUser) -> Self {
        Self {
            first_name: u.first_name,
            last_name: u.last_name,
            profile_picture: u.profile_picture,
        }
    }
}

/// `GET /api/view/me`.
#[derive(Serialize)]
pub struct Me {
    /// The plain uuid, straight from the verified claim, so it always resolves.
    ///
    /// Hyphenated, which is what the GraphQL `uuid` scalar takes on a filter
    /// (`owner_id: { eq: … }`). A `user(id:)` *lookup* needs it wrapped as
    /// `u'<uuid>'` instead — the frontend's `gqlRecordId()` does that.
    pub id: Uuid,
    /// `None` only in the moment between registering and the projection catching up.
    /// Nullable by design: the id is what callers actually need, and making this a
    /// 404 would turn a millisecond of lag into a broken sign-up flow.
    pub profile: Option<Profile>,
}
