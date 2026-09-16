use chrono::{DateTime, Utc};
use diesel::prelude::*;
use uuid::Uuid;

use super::spot::{HostSpotProjection, PublicSpotProjection, SpotCardProjection};
use super::user::UserPublicProjection;
use crate::general_models::booking::Booked;
use crate::schema::view::{booking, spot};

/// `GET /api/view/public/spots/{id}` — the child half. Statement 2 of 2.
///
/// **The public availability answer**, and nothing more: which slots are taken and until
/// when. That is deliberately readable by a prospective renter — it replaced a
/// denormalized `spot.booked` map, and somebody has to be able to see that a slot is
/// gone without being told by whom.
///
/// `spot_id` is here to be the foreign key rather than to be read: `belongs_to` needs the
/// column on the struct, and the response drops it.
///
/// Renter, amount and hold expiry are not `None` here, they are **not selected** — see
/// the module docs.
// `Identifiable` is what supplies `HasTable`, which is what `belonging_to` is called
// through — it is here for that, not because anything looks this row up by id.
#[derive(Debug, Clone, Queryable, Selectable, Identifiable, Associations)]
#[diesel(table_name = booking)]
#[diesel(check_for_backend(diesel::pg::Pg))]
// The foreign key MUST be spelled out. `belongs_to`'s default is
// `<lowercased parent type>_id`, which here would be `public_spot_projection_id`.
#[diesel(belongs_to(PublicSpotProjection, foreign_key = spot_id))]
pub struct PublicBookingProjection {
    pub id: Uuid,
    pub spot_id: Uuid,
    pub booked: Booked,
    pub status: String,
    pub ends_at: DateTime<Utc>,
}

/// `GET /api/view/host/spots/{id}/bookings` — one row of the host's list.
///
/// The same rows as [`PublicBookingProjection`] with the scoped columns added: reaching
/// this type means the parent statement already matched `host_id = caller`, so there is
/// no second check and nothing cut afterwards. `license_plate` is selected here and in
/// [`RenterBookingProjection`] — the two parties to the booking — and in neither of the
/// public reads.
///
/// **No `ends_at`.** It orders the statement and bounds it to one tab, and nothing
/// renders it — a projection selects what a row reads, not what its `WHERE` mentions.
/// `booked` stays, because the row's date line *and* its slot count are both folds
/// over it, and it is also what lets the detail drawer open without a second request.
#[derive(Debug, Clone, Queryable, Selectable, Identifiable, Associations)]
#[diesel(table_name = booking)]
#[diesel(check_for_backend(diesel::pg::Pg))]
#[diesel(belongs_to(HostSpotProjection, foreign_key = spot_id))]
pub struct HostBookingListProjection {
    pub id: Uuid,
    pub spot_id: Uuid,
    pub booked: Booked,
    pub license_plate: String,
    pub status: String,
    /// EUR cents.
    pub amount: i64,
    /// `None` while the renter has not been projected here yet.
    #[diesel(embed)]
    pub renter: Option<UserPublicProjection>,
}

/// Every read in the `renter` namespace: the list, the next-up card and the by-id read.
///
/// One projection for three routes because they are the same rows under the same join,
/// differing only in `WHERE` and `LIMIT`. They do **not** share a response — the next-up
/// card renders four fields and says so in `NextBookingResponse`.
///
/// `HasQuery` earns its place here and nowhere else in this module: `booking ⟕ spot` is
/// identical for all three, so the call site is `RenterBookingProjection::query()`
/// instead of repeating the join. **It cannot hold the scope** — `base_query` is a
/// static expression with no arguments — so `renter_id = caller` stays a `.filter()` at
/// each call site. It shortens the join, not the authorization.
#[derive(Debug, Clone, HasQuery)]
#[diesel(table_name = booking)]
#[diesel(base_query = booking::table.left_join(spot::table.on(spot::id.eq(booking::spot_id))))]
pub struct RenterBookingProjection {
    pub id: Uuid,
    pub status: String,
    /// EUR cents. Unscoped: every row this is built from matched `renter_id = caller`.
    pub amount: i64,
    pub booked: Booked,
    /// The car the renter said they would bring, so the app can tell them again.
    pub license_plate: String,
    pub ends_at: DateTime<Utc>,
    /// `'spot_unavailable'` is how a renter's row says the *host* withdrew, rather than
    /// showing the same bare "cancelled" they would see for their own doing.
    pub cancel_reason: Option<String>,
    /// `None` while the spot has not been projected here yet. A *deleted* spot still
    /// resolves — the row survives precisely so a past booking keeps a title.
    #[diesel(embed)]
    pub spot: Option<SpotCardProjection>,
}
