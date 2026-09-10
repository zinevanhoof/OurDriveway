use diesel::prelude::*;
use uuid::Uuid;

/// A person as anyone may see them: enough to render a name and a face.
///
/// **The one projection here that is shared across routes**, and the one that must never
/// quietly grow a column. `app_user` is the only table in the read model with a
/// field-level scope — `email`, `license_plates` and `country` belong to their host
/// alone — so this is the type whose job is to keep not carrying them. Everything else
/// in this module is per-route and duplicates columns freely, because a spot has no
/// column visible to one reader and not another.
///
/// It is `#[diesel(embed)]`ed as a spot's host and as a booking's renter, both of which
/// are read by callers who are neither.
#[derive(Debug, Clone, Queryable, Selectable)]
#[diesel(table_name = crate::schema::view::app_user)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct UserPublicProjection {
    pub id: Uuid,
    pub first_name: String,
    pub last_name: String,
    pub profile_picture: Option<String>,
}

/// The caller's own row, whole. `GET /api/view/account`.
///
/// Reached by `id = caller` and nothing else, which is the `account` namespace's whole
/// predicate — so the three scoped columns are selected here rather than cut out of a
/// wider read afterwards.
///
/// No `email_verified` and no password hash: neither is a column on the read model's
/// `app_user` at all, which is the first thing deciding what can leak from a table every
/// caller can read.
#[derive(Debug, Clone, Queryable, Selectable)]
#[diesel(table_name = crate::schema::view::app_user)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct AccountProjection {
    #[diesel(embed)]
    pub public: UserPublicProjection,
    /// Not an `Option`. Every `app_user` row is created by `UserRegistered`, whose
    /// `email` is a `String`; `UserUpdated` only ever reaches
    /// `ViewUserRepository::patch`, which is an `UPDATE` that cannot create a row. So a
    /// projected row without an address does not exist, and `Option` would mean nothing.
    pub email: String,
    /// Here rather than on [`UserPublicProjection`] only because no screen shows another
    /// person's plates. A host recognising the car on their driveway reads it off the
    /// booking, not off a profile.
    pub license_plates: Vec<String>,
    /// ISO 3166-1 alpha-2, `None` until the profile screen sets it.
    ///
    /// Scoped like `email` and unlike `license_plates`: where somebody banks is nobody
    /// else's business. It is projected at all because the profile form renders its
    /// current value, and because the withdraw screen needs to know whether there is one
    /// before Stripe refuses to open an account without it.
    pub country: Option<String>,
}
