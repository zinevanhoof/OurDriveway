use diesel::prelude::*;
use uuid::Uuid;

use super::user::UserPublicProjection;
use crate::general_models::spot::{Address, Availability};

/// `GET /api/view/public/spots/{id}` — one active spot as a prospective renter sees it.
///
/// **A spot never carries its bookings.** The taken slots are
/// `GET /public/spots/{id}/bookings`, a separate route the booking form reads when it
/// opens. `Identifiable` stays because that route's statement is
/// `PublicBookingProjection::belonging_to(&spot)` — it proves the spot is active with
/// this read first, then reads the children off it.
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
    /// The zone every `booked` map on this spot is expressed in. Without it a client
    /// cannot tell which slots are in the past.
    pub timezone: String,
    /// `None` while the host's `UserRegistered` has not been projected here. An absent
    /// join, not a permission decision.
    #[diesel(embed)]
    pub host: Option<UserPublicProjection>,
}

/// `GET /api/view/public/spots/nearby` — one map pin.
///
/// **The one list projection that stays.** The host lists read their full type now,
/// because what a narrower row saved there was a few hundred bytes on a page of twenty.
/// A map asks for up to 500 of these, and this skips the host join and `address` on
/// every one of them.
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

/// `GET /api/view/host/spots` and `/host/spots/{id}` — a spot as its host sees it.
///
/// Duplicates most of [`PublicSpotProjection`]'s columns and that is the point: `spot`
/// has no field-level scoping, so the difference between these two is which columns a
/// *screen* reads, not which a caller may see. `description` and `active` are here
/// because the edit form seeds from them and the manage screen draws the live switch off
/// them; the host join is not, because a host does not need their own name told back.
///
/// One type for the list and the by-id read. The list used to have its own narrower
/// projection without `availability`, `description` and `timezone`; measured, that was
/// about 470 bytes a row, and not worth a second type once the list is paged.
///
/// `Identifiable` because the host's booking reads are `belonging_to` it — the spot is
/// read first, by `host_id = caller`, and that read is the ownership proof.
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

/// `GET /api/view/renter/spots` and `/renter/spots/{id}` — a spot the caller has booked.
///
/// Reached only through a booking of the caller's on it, and **not** through `active` or
/// `deleted`: a past booking has to keep its spot after the host paused or removed the
/// listing, which is exactly when the public read stops answering.
///
/// No `availability`: a renter looking at a spot they booked is not booking it from
/// here. `host` is here because the detail sheet shows who they are parking with.
///
/// `timezone` is not decoration: `booked` holds bare wall-clock strings in this zone, so
/// without it "today", "upcoming" and "active now" would be answered in the *viewer's*
/// zone instead — wrong for anyone booking abroad.
#[derive(Debug, Clone, Queryable, Selectable)]
#[diesel(table_name = crate::schema::view::spot)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct RenterSpotProjection {
    pub id: Uuid,
    pub title: String,
    /// EUR cents.
    pub price_per_hour: i64,
    pub images: Vec<String>,
    pub address: Address,
    pub timezone: String,
    /// `None` while the host has not been projected here yet.
    #[diesel(embed)]
    pub host: Option<UserPublicProjection>,
}
