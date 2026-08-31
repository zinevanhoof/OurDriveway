use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use super::MaybeJoined;
use super::spot::SpotCard;
use super::user::PublicViewUser;
use crate::general_models::booking::Booked;

/// A booking as any reader may see it: **the public availability answer.**
///
/// Which booking, which slots, until when, and whether it still blocks. That is
/// deliberately readable by a prospective renter — it replaced a denormalized
/// `spot.booked` map, so availability is a query over these rows and someone has to be
/// able to see that a slot is taken without being told by whom.
///
/// Renter, amount and hold expiry are not `None` here; they are **not selected**. The
/// statement that builds this projection also filters to `status IN ('reserved',
/// 'confirmed')`, so a released or cancelled booking is not a row this type is ever
/// built from.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct PublicViewBooking {
    pub id: Uuid,
    #[sqlx(json)]
    pub booked: Booked,
    pub status: String,
    pub ends_at: DateTime<Utc>,
}

/// A booking as a party to it — the renter or the host — sees it.
///
/// Reaching this type is the permission decision, so nothing here is `Option` for a
/// permission reason. `hold_until` is `None` because the hold was settled;
/// `cancel_reason` because it was not cancelled; `rating` because nobody rated yet.
///
/// Replaces both halves of what `SpotBooking` and `BookingDetail` used to express: the
/// spot's manage screen and the by-id read return the same thing, because the caller is
/// a party in both cases.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct OwnerViewBooking {
    #[sqlx(flatten)]
    #[serde(flatten)]
    pub public: PublicViewBooking,
    pub spot_id: Uuid,
    pub renter_id: Uuid,
    pub owner_id: Uuid,
    /// `None` while the renter has not been projected here yet.
    #[sqlx(flatten)]
    pub renter: MaybeJoined<PublicViewUser>,
    /// EUR cents.
    pub amount: i64,
    /// Lets the renter's own checkout show a countdown. A lapsed hold stops blocking
    /// when the sweeper publishes `Released`, not because a reader learned to skip it.
    pub hold_until: Option<DateTime<Utc>>,
    pub release_reason: Option<String>,
    /// `'spot_unavailable'` is how a renter's row says the *host* withdrew, rather than
    /// showing the same bare "cancelled" they would see for their own doing.
    pub cancel_reason: Option<String>,
    /// The renter's score after the trip. `None` until they rate.
    pub rating: Option<i32>,
    pub created_at: DateTime<Utc>,
}

/// `GET /api/view/me/bookings` — the renter's own list.
///
/// Unscoped, unlike [`PublicViewBooking`]: every row this is built from matched
/// `renter_id = $1`, so there is nothing here the caller may not see.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct BookingListItem {
    pub id: Uuid,
    pub status: String,
    /// EUR cents.
    pub amount: i64,
    #[sqlx(json)]
    pub booked: Booked,
    pub ends_at: DateTime<Utc>,
    pub cancel_reason: Option<String>,
    pub spot_id: Uuid,
    /// `None` while the spot has not been projected here yet. A *deleted* spot still
    /// resolves — the row survives precisely so a past booking keeps a title.
    #[sqlx(flatten)]
    pub spot: MaybeJoined<SpotCard>,
}
