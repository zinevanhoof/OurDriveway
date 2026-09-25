use chrono::{DateTime, Utc};
use diesel::prelude::*;
use uuid::Uuid;

use super::spot::{HostSpotProjection, PublicSpotProjection};
use super::user::UserPublicProjection;
use crate::general_models::booking::Booked;
use crate::general_models::spot::Address;
use crate::schema::view::{booking, spot};

/// `GET /api/view/public/spots/{id}/bookings` — the taken slots on one spot.
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

/// `GET /api/view/host/spots/{id}/bookings` — one booking on the host's own spot.
///
/// The same rows as [`PublicBookingProjection`] with the scoped columns added: reaching
/// this type means the parent statement already matched `host_id = caller`, so there is
/// no second check and nothing cut afterwards. `license_plate` is selected here and in
/// [`RenterBookingProjection`] — the two parties to the booking — and in neither of the
/// public reads.
///
/// One type for the preview and the paged list. A narrower list type used to leave out
/// `ends_at`, which was about forty bytes a row.
#[derive(Debug, Clone, Queryable, Selectable, Identifiable, Associations)]
#[diesel(table_name = booking)]
#[diesel(check_for_backend(diesel::pg::Pg))]
#[diesel(belongs_to(HostSpotProjection, foreign_key = spot_id))]
pub struct HostBookingProjection {
    pub id: Uuid,
    pub spot_id: Uuid,
    pub booked: Booked,
    pub license_plate: String,
    pub status: String,
    pub ends_at: DateTime<Utc>,
    /// EUR cents.
    pub amount: i64,
    /// `None` while the renter has not been projected here yet.
    #[diesel(embed)]
    pub renter: Option<UserPublicProjection>,
}

/// Every booking read in the `renter` namespace: the paged list, the next-up card and the
/// by-id read.
///
/// One projection for three routes because they are the same rows under the same join,
/// differing only in `WHERE` and `LIMIT`. They do **not** share a response — the next-up
/// card renders fewer fields and says so in `NextBookingResponse`.
///
/// ## EXCEPTION: this projection embeds its spot
///
/// **Everywhere else a spot and its bookings are separate reads** — no spot response
/// carries bookings, and no other booking response carries a spot. This one does, and
/// it is the only one.
///
/// The reason is the renter's booking list: every row renders its spot's title, photo
/// and address, and its date line needs the spot's time zone. Fetching the spot
/// separately would mean one request per distinct spot per page, and rows that render
/// without a title until those land. The embed is a small card — five columns, no
/// `availability`, no host — so it does not repeat an expensive parent per row.
///
/// Do not copy this to another projection without the same argument. The full spot, with
/// its host, is still its own read at `/renter/spots/{id}`.
///
/// `HasQuery` holds the `booking ⟕ spot` join because all three reads use it. **It cannot
/// hold the scope** — `base_query` is a static expression with no arguments — so
/// `renter_id = caller` stays a `.filter()` at each call site. It shortens the join, not
/// the authorization.
#[derive(Debug, Clone, HasQuery)]
#[diesel(table_name = booking)]
#[diesel(base_query = booking::table.left_join(spot::table.on(spot::id.eq(booking::spot_id))))]
pub struct RenterBookingProjection {
    pub id: Uuid,
    pub spot_id: Uuid,
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
    /// **The exception** — see above. `None` while the spot has not been projected here
    /// yet. A *deleted* spot still resolves: the row survives precisely so a past booking
    /// keeps a title.
    #[diesel(embed)]
    pub spot: Option<RenterBookingSpotProjection>,
}

/// The spot card embedded in [`RenterBookingProjection`], and nowhere else.
///
/// Only what a booking row draws: a title, one photo, an address, and the zone its
/// `booked` wall-clock strings are in. Without `timezone`, "today", "upcoming" and
/// "active now" would be answered in the *viewer's* zone — wrong for anyone booking
/// abroad.
#[derive(Debug, Clone, Queryable, Selectable)]
#[diesel(table_name = spot)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct RenterBookingSpotProjection {
    pub id: Uuid,
    pub title: String,
    pub images: Vec<String>,
    pub address: Address,
    pub timezone: String,
}

/// How a host's bookings add up — for one person, or for one of their spots.
///
/// One aggregate row, not a group of columns, so `Queryable` alone, like
/// `wallet::BalanceProjection`. Derived on every read, never stored: "completed" is a
/// fact about the clock, and no event announces it for a projector to count.
#[derive(Debug, Clone, Queryable)]
pub struct BookingStatsProjection {
    /// Confirmed bookings that are over. A cancelled one is not a booking received, and
    /// one still to come has not happened yet.
    pub bookings: i64,
    /// The sum of every rating given, `None` when there are none. Sum and count rather
    /// than `avg`, which Postgres answers as `numeric`.
    pub rating_sum: Option<i64>,
    /// How many ratings there are.
    pub ratings: i64,
}
