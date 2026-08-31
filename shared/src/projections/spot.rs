use serde::Serialize;
use uuid::Uuid;

use super::MaybeJoined;
use super::booking::{OwnerViewBooking, PublicViewBooking};
use super::user::PublicViewUser;
use crate::general_models::spot::{Address, Availability};

/// The columns of one spot, shared by both audiences.
///
/// [`PublicViewSpot`] and [`OwnerViewSpot`] select the *same* spot columns — they differ
/// in their `WHERE` and in which booking projection they nest. This holds the shared
/// half so a new spot column is added once rather than twice, which is the failure mode
/// two near-identical thirteen-field structs would have.
///
/// It is `#[serde(flatten)]`ed by both, so the JSON stays one flat object.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct SpotFields {
    pub id: Uuid,
    pub owner_id: Uuid,
    /// `None` while the owner's `UserRegistered` has not been projected here. An absent
    /// join, not a permission decision — see [`MaybeJoined`].
    #[sqlx(flatten)]
    pub owner: MaybeJoined<PublicViewUser>,
    pub title: String,
    /// Genuinely optional: `CreateSpotRequest` does not require one.
    pub description: Option<String>,
    /// EUR cents.
    pub price_per_hour: i64,
    pub images: Vec<String>,
    pub lng: f64,
    pub lat: f64,
    pub active: bool,
    #[sqlx(json)]
    pub address: Address,
    #[sqlx(json)]
    pub availability: Availability,
    /// The zone `booked` is expressed in. Without it a client cannot tell which slots
    /// are in the past.
    pub timezone: String,
}

/// `GET /api/view/spots/:id` — one spot as a prospective renter sees it.
///
/// Reached only for an `active` spot. The bookings are the public availability answer:
/// which slots are taken and until when, with no renter, no amount and no hold expiry.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct PublicViewSpot {
    #[sqlx(flatten)]
    #[serde(flatten)]
    pub spot: SpotFields,
    /// Filled by `ViewSpotRepository::find_public_by_id`'s second statement, never by
    /// the SELECT — `#[sqlx(skip)]` defaults it so the statement need not mention it.
    #[sqlx(skip)]
    pub bookings: Vec<PublicViewBooking>,
}

/// `GET /api/view/spots/:id/manage` — one spot as its host sees it.
///
/// Same columns, reached only by the owner, and its bookings carry the renter, the
/// amount and the hold expiry. An inactive spot resolves here and nowhere else, which
/// is what the live switch is for.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct OwnerViewSpot {
    #[sqlx(flatten)]
    #[serde(flatten)]
    pub spot: SpotFields,
    /// Filled by `ViewSpotRepository::find_owner_by_id`'s second statement.
    #[sqlx(skip)]
    pub bookings: Vec<OwnerViewBooking>,
}

/// The list shape: map pins and the host's own list.
///
/// Drops `description`, `timezone` and the owner join, which have no business on a few
/// hundred map pins. `availability` stays despite its size — the map filters on weekday
/// and time slot client-side, and that fold is not something a query can express.
///
/// No row struct behind it: `#[sqlx(json)]` decodes `address` and `availability`
/// straight into their stored types.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct SpotListItem {
    pub id: Uuid,
    pub title: String,
    /// EUR cents.
    pub price_per_hour: i64,
    pub images: Vec<String>,
    /// The host's live switch. Only ever `false` in the owner's own list — the radius
    /// query filters inactive spots out.
    pub active: bool,
    pub lng: f64,
    pub lat: f64,
    #[sqlx(json)]
    pub address: Address,
    #[sqlx(json)]
    pub availability: Availability,
}

/// Just enough of a spot to render a booking card.
///
/// Only ever nested inside [`super::booking::BookingListItem`], so the `spot_*` renames
/// pin it to that one join site at no cost — the reuse argument that keeps
/// [`PublicViewUser`] on bare-ish aliases does not apply.
///
/// `id` reads the booking's own `spot_id` column rather than a joined one, so it
/// resolves even when the spot has not been projected yet — but the join still has to
/// find `spot_title` for [`MaybeJoined`] to hand back a card at all.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct SpotCard {
    #[sqlx(rename = "spot_id")]
    pub id: Uuid,
    #[sqlx(rename = "spot_title")]
    pub title: String,
    #[sqlx(rename = "spot_images")]
    pub images: Vec<String>,
    /// Not decoration: `booked` holds bare wall-clock strings in this zone, so without
    /// it "today", "upcoming" and "active now" would be answered in the *viewer's* zone
    /// instead — wrong for anyone booking abroad.
    #[sqlx(rename = "spot_timezone")]
    pub timezone: String,
    #[sqlx(rename = "spot_address")]
    #[sqlx(json)]
    pub address: Address,
}
