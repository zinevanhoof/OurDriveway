use serde::Serialize;
use uuid::Uuid;

/// A person as anyone may see them: enough to render a name and a face.
///
/// **Never carries `email`.** This is the shape a spot's owner and a booking's renter
/// are embedded as, and both are visible to callers who are neither.
///
/// Columns are read as `user_*` at every site, join or not — see the module docs. The
/// aliases are what let one type serve owner-on-spot and renter-on-booking without
/// colliding with the parent's own `id`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct PublicViewUser {
    #[sqlx(rename = "user_id")]
    pub id: Uuid,
    #[sqlx(rename = "user_first_name")]
    pub first_name: String,
    #[sqlx(rename = "user_last_name")]
    pub last_name: String,
    #[sqlx(rename = "user_profile_picture")]
    pub profile_picture: Option<String>,
}

/// The caller's own profile. Carries `email`, which [`PublicViewUser`] does not.
///
/// No `email_verified` and no password hash — neither is a column on the read model's
/// `app_user` at all, which is the first thing deciding what can possibly leak from a
/// table every caller can read.
///
/// `license_plates` is deliberately on the *public* side of nothing: it is here rather
/// than on [`PublicViewUser`] only because no screen shows another person's plates. A
/// host recognising the car on their driveway reads it from the booking, not from a
/// profile.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct OwnerViewUser {
    #[sqlx(flatten)]
    #[serde(flatten)]
    pub public: PublicViewUser,
    /// Not an `Option`. Every `app_user` row is created by `UserRegistered`, whose
    /// `email` is a `String`; `UserUpdated` only ever reaches `ViewUserRepository::patch`,
    /// which is an `UPDATE` that cannot create a row. So a projected row without an
    /// address does not exist, and `Option` here would mean nothing.
    #[sqlx(rename = "user_email")]
    pub email: String,
    #[sqlx(rename = "user_license_plates")]
    pub license_plates: Vec<String>,
    /// ISO 3166-1 alpha-2, `None` until the profile screen sets it.
    ///
    /// On this side rather than [`PublicViewUser`], and unlike `license_plates`: where
    /// somebody banks is nobody else's business. It is here at all because the profile
    /// form has to render its current value, and because the withdraw screen needs to
    /// know whether it has one before Stripe refuses to open an account without it.
    #[sqlx(rename = "user_country")]
    pub country: Option<String>,
}

/// `GET /api/view/me`.
///
/// The id is the plain uuid straight from the verified claim, so it always resolves.
/// `profile` is `None` only in the moment between registering and the projection
/// catching up — nullable by design, because the id is what callers actually need and a
/// 404 would turn a millisecond of lag into a broken sign-up flow.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Me {
    pub id: Uuid,
    pub profile: Option<OwnerViewUser>,
}
