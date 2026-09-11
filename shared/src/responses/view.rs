//! What view-service **answers with**. One response per route, named for the route's
//! query where it has one (`NearbyQuery` → [`NearbyResponse`]), for its handler where it
//! does not.
//!
//! Serde only — diesel never sees a type in this file. Each one is built from a
//! projection in [`crate::projections`] by a `From` impl at the bottom of its section,
//! and carries **exactly** the fields the frontend reads: the component templates are the
//! source, and a field nothing renders is a field that is not here.
//!
//! ## Why this exists even where it is identical to its projection
//!
//! `WalletTransactionResponse` is field-for-field its projection today. It is still
//! written out, because the two have different jobs and therefore different reasons to
//! change: a projection changes when a statement needs another column, a response changes
//! when a client needs another field, and those are not the same event. Without the
//! split, adding a column to a read silently widens the wire contract — which is how
//! `email` ends up on a public profile.
//!
//! It is also what replaces `#[sqlx(skip)]`, which has no diesel equivalent: a second
//! statement's `Vec` is a response field, never a projection field.
//!
//! `#[serde(rename_all = "camelCase")]` on every struct. `apps/frontend/src/types/view.ts`
//! mirrors these names; projection names never cross the wire.

use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use crate::general_models::booking::Booked;
use crate::general_models::spot::{Address, Availability};
use crate::projections::{
    booking::{HostBookingProjection, PublicBookingProjection, RenterBookingProjection},
    spot::{
        HostSpotListProjection, HostSpotProjection, PublicSpotPinProjection, PublicSpotProjection,
        SpotCardProjection,
    },
    user::{AccountProjection, UserPublicProjection},
    wallet::{BalanceProjection, WalletTransactionProjection},
};

// ─── people ─────────────────────────────────────────────────────────────────

/// A person as anyone may see them. Never carries an email.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserPublicResponse {
    pub id: Uuid,
    pub first_name: String,
    pub last_name: String,
    pub profile_picture: Option<String>,
}

impl From<UserPublicProjection> for UserPublicResponse {
    fn from(u: UserPublicProjection) -> Self {
        Self {
            id: u.id,
            first_name: u.first_name,
            last_name: u.last_name,
            profile_picture: u.profile_picture,
        }
    }
}

/// `GET /api/view/account` — the caller's own profile.
///
/// `id` comes from the verified claim rather than from a row, so it always resolves;
/// `profile` is `None` only in the moment between registering and the projection catching
/// up. Nullable by design: a 404 there would turn a millisecond of lag into a broken
/// sign-up flow.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountResponse {
    pub id: Uuid,
    pub profile: Option<AccountProfileResponse>,
}

/// The profile half of [`AccountResponse`].
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountProfileResponse {
    pub first_name: String,
    pub last_name: String,
    pub profile_picture: Option<String>,
    pub email: String,
    pub license_plates: Vec<String>,
    /// ISO 3166-1 alpha-2, or `null` until the profile screen sets it.
    pub country: Option<String>,
}

impl From<AccountProjection> for AccountProfileResponse {
    fn from(a: AccountProjection) -> Self {
        Self {
            first_name: a.public.first_name,
            last_name: a.public.last_name,
            profile_picture: a.public.profile_picture,
            email: a.email,
            license_plates: a.license_plates,
            country: a.country,
        }
    }
}

// ─── public spots ───────────────────────────────────────────────────────────

/// `GET /api/view/public/spots/{id}` — one active spot as a prospective renter sees it.
///
/// `bookings` is the second statement's rows and is a response field by construction:
/// there is no column on `spot` for it to be a projection field of.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicSpotResponse {
    pub id: Uuid,
    pub title: String,
    /// EUR cents.
    pub price_per_hour: i64,
    pub images: Vec<String>,
    pub address: Address,
    pub availability: Availability,
    /// The zone `bookings[].booked` is expressed in.
    pub timezone: String,
    /// `null` while the host has not been projected here yet — an absent join.
    pub host: Option<UserPublicResponse>,
    /// **The availability answer**: which slots are taken and until when. No renter, no
    /// amount, no hold expiry — not nulled, not selected.
    pub bookings: Vec<PublicBookingResponse>,
}

impl PublicSpotResponse {
    pub fn new(spot: PublicSpotProjection, bookings: Vec<PublicBookingProjection>) -> Self {
        Self {
            id: spot.id,
            title: spot.title,
            price_per_hour: spot.price_per_hour,
            images: spot.images,
            address: spot.address,
            availability: spot.availability,
            timezone: spot.timezone,
            host: spot.host.map(Into::into),
            bookings: bookings.into_iter().map(Into::into).collect(),
        }
    }
}

/// One booking on a public spot page.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicBookingResponse {
    pub id: Uuid,
    pub booked: Booked,
    pub status: String,
    pub ends_at: DateTime<Utc>,
}

impl From<PublicBookingProjection> for PublicBookingResponse {
    fn from(b: PublicBookingProjection) -> Self {
        Self {
            id: b.id,
            booked: b.booked,
            status: b.status,
            ends_at: b.ends_at,
        }
    }
}

/// `GET /api/view/public/spots/nearby` — one map pin, answering view-service's
/// `NearbyQuery`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NearbyResponse {
    pub id: Uuid,
    pub title: String,
    /// EUR cents.
    pub price_per_hour: i64,
    pub images: Vec<String>,
    pub lng: f64,
    pub lat: f64,
    /// The map's weekday-and-time filter is a fold over this that no query expresses.
    pub availability: Availability,
}

impl From<PublicSpotPinProjection> for NearbyResponse {
    fn from(s: PublicSpotPinProjection) -> Self {
        Self {
            id: s.id,
            title: s.title,
            price_per_hour: s.price_per_hour,
            images: s.images,
            lng: s.lng,
            lat: s.lat,
            availability: s.availability,
        }
    }
}

// ─── host ───────────────────────────────────────────────────────────────────

/// `GET /api/view/host/spots/{id}` — one spot as its host sees it.
///
/// Serves the manage screen and the edit form. They render different fields, not
/// different permissions, so they are one route: the form needs the bookings to stop a
/// host removing a slot someone has taken, the screen needs the same rows with their
/// renters attached.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostSpotResponse {
    pub id: Uuid,
    pub title: String,
    pub description: Option<String>,
    /// EUR cents.
    pub price_per_hour: i64,
    pub images: Vec<String>,
    /// The live switch. An inactive spot resolves here and nowhere else.
    pub active: bool,
    pub address: Address,
    pub availability: Availability,
    pub timezone: String,
    pub bookings: Vec<HostBookingResponse>,
}

impl HostSpotResponse {
    pub fn new(spot: HostSpotProjection, bookings: Vec<HostBookingProjection>) -> Self {
        Self {
            id: spot.id,
            title: spot.title,
            description: spot.description,
            price_per_hour: spot.price_per_hour,
            images: spot.images,
            active: spot.active,
            address: spot.address,
            availability: spot.availability,
            timezone: spot.timezone,
            bookings: bookings.into_iter().map(Into::into).collect(),
        }
    }
}

/// One booking on a host's own spot. Carries the renter and the amount, because reaching
/// it already proved ownership.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostBookingResponse {
    pub id: Uuid,
    pub booked: Booked,
    pub status: String,
    pub ends_at: DateTime<Utc>,
    /// EUR cents.
    pub amount: i64,
    /// `null` while the renter has not been projected here yet.
    pub renter: Option<UserPublicResponse>,
}

impl From<HostBookingProjection> for HostBookingResponse {
    fn from(b: HostBookingProjection) -> Self {
        Self {
            id: b.id,
            booked: b.booked,
            status: b.status,
            ends_at: b.ends_at,
            amount: b.amount,
            renter: b.renter.map(Into::into),
        }
    }
}

/// `GET /api/view/host/spots` — one row of the host's own list.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostSpotListItemResponse {
    pub id: Uuid,
    pub title: String,
    /// EUR cents.
    pub price_per_hour: i64,
    pub images: Vec<String>,
    /// `false` is a paused listing, which looks identical to a live one otherwise.
    pub active: bool,
    pub address: Address,
}

impl From<HostSpotListProjection> for HostSpotListItemResponse {
    fn from(s: HostSpotListProjection) -> Self {
        Self {
            id: s.id,
            title: s.title,
            price_per_hour: s.price_per_hour,
            images: s.images,
            active: s.active,
            address: s.address,
        }
    }
}

/// `GET /api/view/host/balance` — what the host has to withdraw, and what is ripening.
///
/// Two figures where [`BalanceProjection`] computes four. `earned_cents` and
/// `paid_out_cents` are the arithmetic behind `available_cents` and no screen renders
/// them, so they stop at the repository.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BalanceResponse {
    /// Withdrawable now: settled income minus what has already been taken out.
    pub available_cents: i64,
    /// Earned but not settled yet — what `available_cents` will grow by.
    pub pending_cents: i64,
}

impl From<BalanceProjection> for BalanceResponse {
    fn from(b: BalanceProjection) -> Self {
        Self {
            available_cents: b.available_cents,
            pending_cents: b.pending_cents,
        }
    }
}

// ─── renter ─────────────────────────────────────────────────────────────────

/// `GET /api/view/renter/bookings` and `/renter/bookings/{id}` — one of the caller's own
/// bookings, whole.
///
/// Unscoped: every row it is built from matched `renter_id = caller`, so there is nothing
/// here the caller may not see.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RenterBookingResponse {
    pub id: Uuid,
    pub status: String,
    /// EUR cents.
    pub amount: i64,
    pub booked: Booked,
    pub ends_at: DateTime<Utc>,
    /// `'spot_unavailable'` means the host withdrew, not that you cancelled.
    pub cancel_reason: Option<String>,
    /// `null` while the spot has not been projected here yet. A *deleted* spot still
    /// resolves — the row survives so a past booking keeps a title.
    pub spot: Option<SpotCardResponse>,
}

impl From<RenterBookingProjection> for RenterBookingResponse {
    fn from(b: RenterBookingProjection) -> Self {
        Self {
            id: b.id,
            status: b.status,
            amount: b.amount,
            booked: b.booked,
            ends_at: b.ends_at,
            cancel_reason: b.cancel_reason,
            spot: b.spot.map(Into::into),
        }
    }
}

/// `GET /api/view/renter/bookings/next` — the home screen's next-up card.
///
/// **The same projection as [`RenterBookingResponse`], a different response**, which is
/// the clearest case for why every route has one. The card renders a title, a zone, the
/// slots and — once opened into the detail sheet — the amount. It does not render a
/// status, an end instant or a cancel reason, so it is not sent them.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NextBookingResponse {
    pub id: Uuid,
    /// The slots, in `spot_timezone`'s wall clock. What the card's day and time read off.
    pub booked: Booked,
    /// EUR cents. Read by the detail sheet the card opens, not by the card.
    pub amount: i64,
    /// `null` while the spot has not been projected here yet.
    pub spot: Option<SpotCardResponse>,
}

impl From<RenterBookingProjection> for NextBookingResponse {
    fn from(b: RenterBookingProjection) -> Self {
        Self {
            id: b.id,
            booked: b.booked,
            amount: b.amount,
            spot: b.spot.map(Into::into),
        }
    }
}

/// Just enough of a spot to render a booking card.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpotCardResponse {
    pub id: Uuid,
    pub title: String,
    pub images: Vec<String>,
    /// `booked` is wall-clock in this zone; without it "upcoming" is answered wrong.
    pub timezone: String,
    pub address: Address,
}

impl From<SpotCardProjection> for SpotCardResponse {
    fn from(s: SpotCardProjection) -> Self {
        Self {
            id: s.id,
            title: s.title,
            images: s.images,
            timezone: s.timezone,
            address: s.address,
        }
    }
}

// ─── wallet ─────────────────────────────────────────────────────────────────

/// One line of the wallet: a single movement of money involving the caller.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WalletTransactionResponse {
    /// `<uuid>:<kind>`. One payment yields two rows — the charge and its refund.
    pub id: String,
    /// One of `projections::wallet::kind`, which decides the icon. The *direction* is the
    /// sign of `amount_cents`, because a refund is money back to a renter and money away
    /// from a host.
    pub kind: String,
    /// Signed EUR cents, from the caller's point of view: what their balance did.
    pub amount_cents: i64,
    pub occurred_at: DateTime<Utc>,
    /// Host income that has not settled yet, so it is not withdrawable.
    pub pending: bool,
    /// The spot's title. `null` on a payout, and on a spot not projected yet.
    pub title: Option<String>,
    /// The booking's slots, in the spot's own wall clock. `null` on a payout.
    pub booked: Option<Booked>,
    pub timezone: Option<String>,
}

impl WalletTransactionResponse {
    /// `new` and not `From`, because `pending` needs the settlement cutoff and a `From`
    /// has nowhere to take it. The projection carries `settles_at` and `pending_now`;
    /// deciding what they mean is `policy::wallet::pending`'s job, and the route passes
    /// the same `now - SETTLEMENT_SECS` it uses everywhere else.
    pub fn new(t: WalletTransactionProjection, pending: bool) -> Self {
        Self {
            id: t.id,
            kind: t.kind,
            amount_cents: t.amount_cents,
            occurred_at: t.occurred_at,
            pending,
            title: t.title,
            booked: t.booked,
            timezone: t.timezone,
        }
    }
}

/// `GET /api/view/account/wallet?month=YYYY-MM` — one month, which is also one page,
/// answering view-service's `WalletQuery`.
///
/// `next_month` is the cursor: the next older month that holds anything, or `null` at the
/// end of the history. Without it a client would ask for the previous month, get nothing,
/// and either stop — hiding everything behind a quiet month — or walk backwards forever.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WalletResponse {
    /// `"YYYY-MM"`, as asked for.
    pub month: String,
    /// Everything that came in, as a positive figure.
    pub in_cents: i64,
    /// Everything that went out, as a **positive** figure, and deliberately not including
    /// payouts: moving your own money to your own account is not spending it.
    pub out_cents: i64,
    pub next_month: Option<String>,
    /// Newest first.
    pub transactions: Vec<WalletTransactionResponse>,
}
