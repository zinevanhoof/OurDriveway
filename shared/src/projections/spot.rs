use diesel::prelude::*;
use uuid::Uuid;

use super::user::UserPublicProjection;
use crate::general_models::spot::{Address, Availability};

/// `GET /api/view/public/spots/{id}` — the parent half. Statement 1 of 2.
///
/// `Identifiable` is what makes it the parent: [`super::booking::PublicBookingProjection`]
/// `belongs_to` it, so statement 2 is `PublicBookingProjection::belonging_to(&spot)`
/// rather than a hand-written `spot_id = $1`.
///
/// **Two statements, deliberately.** One join would repeat `images`, `address` and
/// `availability` once per booking, and those are the expensive columns. A
/// single-statement form exists and works — a correlated `array_agg` over a row
/// constructor decoded through `Record<(…)>`, built and verified during the diesel
/// migration — and is not used: two statements are cheaper for a large jsonb parent and
/// give parent and children independent cache keys, so making a booking invalidates the
/// availability without refetching the listing.
///
/// No `host_id`, `lng`, `lat`, `active` or `description`: nothing on the detail sheet
/// or the booking form reads them. `active` in particular is not a field because the
/// statement's own `WHERE` already required it — a column saying `true` on every row it
/// can return is a column that answers nothing.
#[derive(Debug, Clone, Queryable, Selectable, Identifiable)]
#[diesel(table_name = crate::schema::view::spot)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct PublicSpotProjection {
    pub id: Uuid,
    pub title: String,
    /// EUR cents.
    pub price_per_hour: i64,
    pub images: Vec<String>,
    pub address: Address,
    pub availability: Availability,
    /// The zone the bookings' `booked` maps are expressed in. Without it a client cannot
    /// tell which slots are in the past.
    pub timezone: String,
    /// `None` while the host's `UserRegistered` has not been projected here. An absent
    /// join, not a permission decision.
    #[diesel(embed)]
    pub host: Option<UserPublicProjection>,
}

/// `GET /api/view/public/spots/nearby` — one map pin.
///
/// `lng`/`lat` and `availability`, which the detail read does not carry, and no
/// `address`: a pin is placed by coordinates and labelled by title and price. The map's
/// weekday-and-time filter is a fold over `availability` that no query expresses, which
/// is why that one column survives onto a few hundred rows.
#[derive(Debug, Clone, Queryable, Selectable)]
#[diesel(table_name = crate::schema::view::spot)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct PublicSpotPinProjection {
    pub id: Uuid,
    pub title: String,
    /// EUR cents.
    pub price_per_hour: i64,
    pub images: Vec<String>,
    pub lng: f64,
    pub lat: f64,
    pub availability: Availability,
}

/// `GET /api/view/host/spots/{id}` — the parent half. Statement 1 of 2.
///
/// Duplicates most of [`PublicSpotProjection`]'s columns and that is the point: `spot`
/// has no field-level scoping, so the difference between these two is which columns a
/// *screen* reads, not which a caller may see. `description` and `active` are here
/// because the edit form seeds from them and the manage screen draws the live switch off
/// them; the host join is not, because a host does not need their own name told back.
#[derive(Debug, Clone, Queryable, Selectable, Identifiable)]
#[diesel(table_name = crate::schema::view::spot)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct HostSpotProjection {
    pub id: Uuid,
    pub title: String,
    /// Genuinely optional: `CreateSpotRequest` does not require one.
    pub description: Option<String>,
    /// EUR cents.
    pub price_per_hour: i64,
    pub images: Vec<String>,
    /// The host's live switch. Only ever read here and in the list — a public read
    /// cannot reach an inactive spot at all.
    pub active: bool,
    pub address: Address,
    pub availability: Availability,
    pub timezone: String,
}

/// `GET /api/view/host/spots` — one row of the host's own list.
///
/// No `availability` and no coordinates: this list is not a map, and `availability` is
/// the largest jsonb column on the table. `active` is here and nowhere in the public
/// reads, because a host's paused listing is exactly what this list has to show.
#[derive(Debug, Clone, Queryable, Selectable)]
#[diesel(table_name = crate::schema::view::spot)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct HostSpotListProjection {
    pub id: Uuid,
    pub title: String,
    /// EUR cents.
    pub price_per_hour: i64,
    pub images: Vec<String>,
    pub active: bool,
    pub address: Address,
}

/// Just enough of a spot to render a booking card, nested in a renter's booking.
///
/// Only ever `#[diesel(embed)]`ed inside [`super::booking::RenterBookingProjection`], off
/// a LEFT JOIN — so it is `Option` there, and resolves for a *deleted* spot, whose row
/// survives precisely so a past booking keeps a title.
///
/// `timezone` is not decoration: `booked` holds bare wall-clock strings in this zone, so
/// without it "today", "upcoming" and "active now" would be answered in the *viewer's*
/// zone instead — wrong for anyone booking abroad.
#[derive(Debug, Clone, Queryable, Selectable)]
#[diesel(table_name = crate::schema::view::spot)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct SpotCardProjection {
    pub id: Uuid,
    pub title: String,
    pub images: Vec<String>,
    pub timezone: String,
    pub address: Address,
}
