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
//! `email` ends up on a public user row.
//!
//! **A spot never carries its bookings, and a booking never carries its spot.** Each is
//! its own route, and a screen that needs both fetches both. **One exception:** a
//! renter's own bookings embed a small card of their spot — see
//! `projections::booking::RenterBookingProjection` for why.
//!
//! `#[serde(rename_all = "camelCase")]` on every struct. `apps/frontend/src/types/view.ts`
//! mirrors these names; projection names never cross the wire.

use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use crate::domain_models::view::notification::NotificationPayload;
use crate::general_models::booking::Booked;
use crate::general_models::spot::{Address, Availability};
use crate::projections::{
    booking::{
        HostBookingProjection, PublicBookingProjection, RenterBookingProjection,
        RenterBookingSpotProjection,
    },
    spot::{
        HostSpotProjection, PublicSpotPinProjection, PublicSpotProjection, RenterSpotProjection,
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

/// `GET /api/view/account` — the caller's own user record.
///
/// `id` comes from the verified claim rather than from a row, so it always resolves;
/// `user` is `None` only in the moment between registering and the projection catching
/// up. Nullable by design: a 404 there would turn a millisecond of lag into a broken
/// sign-up flow.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountResponse {
    pub id: Uuid,
    pub user: Option<AccountUserResponse>,
}

/// The row half of [`AccountResponse`] — the caller as only the caller may see them.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountUserResponse {
    pub first_name: String,
    pub last_name: String,
    pub profile_picture: Option<String>,
    pub email: String,
    pub license_plates: Vec<String>,
    /// ISO 3166-1 alpha-2, or `null` until the edit screen sets it.
    pub country: Option<String>,
}

impl From<AccountProjection> for AccountUserResponse {
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

/// `GET /api/view/public/users/{id}/summary` — a person's reputation as a host.
///
/// Public, and its own route rather than fields on [`UserPublicResponse`]: that type is
/// embedded in list rows — the renter on every host booking — and would compute these on
/// every one of them. A client hides each figure that is zero.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserSummaryResponse {
    /// Confirmed bookings on their spots that are over.
    pub bookings: i64,
    /// The average rating they have received, or `null` when nobody has rated them.
    pub rating: Option<f64>,
    pub ratings: i64,
}

// ─── public spots ───────────────────────────────────────────────────────────

/// `GET /api/view/public/spots/{id}/summary` — a spot's rating, for anyone.
///
/// Rating only: how much a spot has earned and how often it is booked are its host's
/// business (`/host/spots/{id}/summary`). A client hides it when there are no ratings.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpotSummaryResponse {
    /// The average rating, or `null` when nobody has rated this spot.
    pub rating: Option<f64>,
    pub ratings: i64,
}

/// `GET /api/view/public/spots/{id}` — one active spot as a prospective renter sees it.
///
/// No bookings: the taken slots are `GET /public/spots/{id}/bookings`, fetched by the
/// booking form when it opens and by nothing else.
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
    /// The zone every `booked` map on this spot is expressed in.
    pub timezone: String,
    /// `null` while the host has not been projected here yet — an absent join.
    pub host: Option<UserPublicResponse>,
}

impl From<PublicSpotProjection> for PublicSpotResponse {
    fn from(spot: PublicSpotProjection) -> Self {
        Self {
            id: spot.id,
            title: spot.title,
            price_per_hour: spot.price_per_hour,
            images: spot.images,
            address: spot.address,
            availability: spot.availability,
            timezone: spot.timezone,
            host: spot.host.map(Into::into),
        }
    }
}

/// `GET /api/view/public/spots/{id}/bookings` — one slot-taking booking on a spot.
///
/// **The availability answer**: which slots are taken and until when. No renter, no
/// amount, no hold expiry — not nulled, not selected.
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

/// `GET /api/view/host/spots` and `/host/spots/{id}` — a spot as its host sees it.
///
/// Serves the list, the manage screen and the edit form. They render different fields,
/// not different permissions, so they are one type. No bookings: the rows are
/// `GET /host/spots/{id}/bookings` and the taken slots `GET /host/spots/{id}/booked`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostSpotResponse {
    pub id: Uuid,
    pub title: String,
    pub description: Option<String>,
    /// EUR cents.
    pub price_per_hour: i64,
    pub images: Vec<String>,
    /// The live switch. `false` is a paused listing, which looks identical to a live one
    /// otherwise. An inactive spot resolves here and nowhere public.
    pub active: bool,
    pub address: Address,
    pub availability: Availability,
    pub timezone: String,
}

impl From<HostSpotProjection> for HostSpotResponse {
    fn from(spot: HostSpotProjection) -> Self {
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
        }
    }
}

/// `GET /api/view/host/spots?limit=&offset=` — one window of the host's own listings.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostSpotsPageResponse {
    pub spots: Vec<HostSpotResponse>,
    /// The offset to ask for next, or `null` at the end of the list.
    pub next_offset: Option<i64>,
    /// Every listing the host has that is not deleted.
    pub total: i64,
}

/// `GET /api/view/host/spots/{id}/bookings` — one booking on a host's own spot.
///
/// Carries the renter and the amount, because reaching it already proved ownership.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostBookingResponse {
    pub id: Uuid,
    /// The date line and the slot count are both folds over this.
    pub booked: Booked,
    pub license_plate: String,
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
            license_plate: b.license_plate,
            status: b.status,
            ends_at: b.ends_at,
            amount: b.amount,
            renter: b.renter.map(Into::into),
        }
    }
}

/// `GET /api/view/host/spots/{id}/bookings?scope=&status=&limit=&offset=` — one window of
/// them.
///
/// **Limit and offset, not a cursor**, unlike [`WalletResponse`]: the same route serves a
/// two-row preview and a paged list, and a host may want either sorted differently later —
/// an offset survives a change of sort order where a keyset cursor does not. What it does
/// not survive is the list moving underneath it — see the note on
/// `ViewBookingRepository::find_page_for_host_spot`.
///
/// `next_offset` rather than a computed one, for the same reason `next_month` exists: the
/// client asks for what the server said was next and never does the arithmetic itself.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostBookingsPageResponse {
    pub bookings: Vec<HostBookingResponse>,
    /// The offset to ask for next, or `null` at the end of the list.
    pub next_offset: Option<i64>,
    /// Every booking in this scope and status, not just this window.
    pub total: i64,
}

/// `GET /api/view/host/spots/{id}/summary` — how one of the host's own spots is doing.
///
/// The manage screen's tiles. Its own route rather than fields on [`HostSpotResponse`]:
/// the listing changes rarely, these change with every booking, and the spot is also
/// read by screens that show none of them.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostSpotSummaryResponse {
    /// Confirmed bookings on this spot that are over.
    pub bookings: i64,
    /// EUR cents. Succeeded payments on bookings still confirmed, upcoming ones included.
    pub earned_cents: i64,
    /// The average rating, or `null` when nobody has rated this spot.
    pub rating: Option<f64>,
    pub ratings: i64,
}

/// `GET /api/view/host/summary` — the caller's totals as a host: the profile row and the
/// tiles on top of "Your parking spots".
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostSummaryResponse {
    /// Listings that are not deleted, paused ones included.
    pub spots: i64,
    /// Of those, the ones that are live.
    pub active_spots: i64,
    /// Listings with a confirmed booking happening at this moment, on each spot's clock.
    pub booked_now: i64,
    /// Confirmed bookings across all their spots that are over.
    pub bookings: i64,
    /// EUR cents. Succeeded payments on bookings still confirmed, upcoming ones included.
    pub earned_cents: i64,
    /// EUR cents. The same, for payments made this calendar month (UTC).
    pub earned_this_month_cents: i64,
    /// EUR cents. The same, for last calendar month — what "vs last" compares against.
    pub earned_last_month_cents: i64,
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

/// `GET /api/view/renter/spots/{id}` — a spot the caller has booked, whole, with its host.
///
/// Resolves for a paused or deleted listing too: a renter's past booking keeps its spot.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RenterSpotResponse {
    pub id: Uuid,
    pub title: String,
    /// EUR cents.
    pub price_per_hour: i64,
    pub images: Vec<String>,
    pub address: Address,
    /// `booked` on this spot's bookings is wall-clock in this zone.
    pub timezone: String,
    /// `null` while the host has not been projected here yet.
    pub host: Option<UserPublicResponse>,
}

impl From<RenterSpotProjection> for RenterSpotResponse {
    fn from(s: RenterSpotProjection) -> Self {
        Self {
            id: s.id,
            title: s.title,
            price_per_hour: s.price_per_hour,
            images: s.images,
            address: s.address,
            timezone: s.timezone,
            host: s.host.map(Into::into),
        }
    }
}

/// `GET /api/view/renter/bookings` and `/renter/bookings/{id}` — one of the caller's own
/// bookings, whole.
///
/// Unscoped: every row it is built from matched `renter_id = caller`, so there is nothing
/// here the caller may not see.
///
/// **EXCEPTION: carries its spot.** The one booking response that does — see
/// `RenterBookingProjection`. `spot_id` is kept beside it because `spot` is `null` until
/// the spot is projected, and the detail sheet needs the id either way.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RenterBookingResponse {
    pub id: Uuid,
    pub spot_id: Uuid,
    pub status: String,
    /// EUR cents.
    pub amount: i64,
    pub booked: Booked,
    /// The car the renter said they would bring.
    pub license_plate: String,
    pub ends_at: DateTime<Utc>,
    /// `'spot_unavailable'` means the host withdrew, not that you cancelled.
    pub cancel_reason: Option<String>,
    /// The exception. `null` while the spot has not been projected here yet.
    pub spot: Option<RenterBookingSpotResponse>,
}

impl From<RenterBookingProjection> for RenterBookingResponse {
    fn from(b: RenterBookingProjection) -> Self {
        Self {
            id: b.id,
            spot_id: b.spot_id,
            status: b.status,
            amount: b.amount,
            booked: b.booked,
            license_plate: b.license_plate,
            ends_at: b.ends_at,
            cancel_reason: b.cancel_reason,
            spot: b.spot.map(Into::into),
        }
    }
}

/// The spot card on a renter's booking — the exception's wire half.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RenterBookingSpotResponse {
    pub id: Uuid,
    pub title: String,
    pub images: Vec<String>,
    pub address: Address,
    /// `booked` is wall-clock in this zone; without it "upcoming" is answered wrong.
    pub timezone: String,
}

impl From<RenterBookingSpotProjection> for RenterBookingSpotResponse {
    fn from(s: RenterBookingSpotProjection) -> Self {
        Self {
            id: s.id,
            title: s.title,
            images: s.images,
            address: s.address,
            timezone: s.timezone,
        }
    }
}

/// `GET /api/view/renter/bookings?scope=&status=&limit=&offset=` — one window of the
/// caller's own bookings.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RenterBookingsPageResponse {
    pub bookings: Vec<RenterBookingResponse>,
    /// The offset to ask for next, or `null` at the end of the list.
    pub next_offset: Option<i64>,
    /// Every booking in this scope and status, not just this window.
    pub total: i64,
}

/// `GET /api/view/renter/bookings/next` — the home screen's next-up card.
///
/// **The same projection as [`RenterBookingResponse`], a different response**, which is
/// the clearest case for why every route has one. The card renders a title, a zone, the
/// slots and — once opened into the detail sheet — the plate and the amount. It does not
/// render a status, an end instant or a cancel reason, so it is not sent them.
///
/// Carries its spot under the same **exception** as [`RenterBookingResponse`].
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NextBookingResponse {
    pub id: Uuid,
    pub spot_id: Uuid,
    /// The slots, in the spot's wall clock. What the card's day and time read off.
    pub booked: Booked,
    /// Which car to bring — the one thing on this card the renter may have forgotten.
    pub license_plate: String,
    /// EUR cents. Read by the detail sheet the card opens, not by the card.
    pub amount: i64,
    /// The exception. `null` while the spot has not been projected here yet.
    pub spot: Option<RenterBookingSpotResponse>,
}

impl From<RenterBookingProjection> for NextBookingResponse {
    fn from(b: RenterBookingProjection) -> Self {
        Self {
            id: b.id,
            spot_id: b.spot_id,
            booked: b.booked,
            license_plate: b.license_plate,
            amount: b.amount,
            spot: b.spot.map(Into::into),
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

// ─── notifications ──────────────────────────────────────────────────────────

/// `GET /api/view/account/notifications` — one open notification.
///
/// The payload is the stored [`NotificationPayload`] itself, flattened, rather than a
/// response-side copy of every variant: it is already exactly what the drawer renders,
/// and a second enum would double the work of adding a kind for no field of difference.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationResponse {
    #[serde(flatten)]
    pub payload: NotificationPayload,
    /// When it became visible — for a rating prompt, when the booking ended.
    pub at: DateTime<Utc>,
    /// Visible since before the user last opened their notifications. The badge counts
    /// the ones where this is `false`.
    pub seen: bool,
}
