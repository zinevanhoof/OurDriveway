//! Everything the caller reads about themselves.
//!
//! Five routes, and the subject of every one of them is the verified claim: there is no
//! id to extract and none to compare. `SPOTS_OWNED`, `BOOKINGS_RENTED` and `PAYOUTS`
//! each passed the reader's own id as a variable, which was safe only because a table
//! permission clause independently refused everyone else's rows. With the clauses gone
//! an id in a query string would be a request rather than a claim, so the path says
//! `/me` and the handler takes the id from the token.
//!
//! `wallet` takes the one parameter here that is not an identity — which month — and
//! that is exactly why it is safe as a query string.

use axum::{
    Json,
    extract::{Query, State},
    response::IntoResponse,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use shared::{
    error::myerror::{ContextExt, MyResult},
    extractors::authed_jwt::AuthedJwt,
    projections::{user::Me, wallet::WalletMonth},
};

use crate::{
    AppState, policy,
    repository::{
        booking_repository::ViewBookingRepository, spot_repository::ViewSpotRepository,
        user_repository::ViewUserRepository, wallet_repository::WalletRepository,
    },
};

/// `GET /api/view/me` — the caller's own profile, `email` included.
///
/// This endpoint predates the rest of the REST read API and existed because one
/// `FOR select` clause could not be both "only me" and "public": scoping the table to
/// the caller broke spot-owner names on the map, and leaving it world-readable meant
/// `users { id }` returned everyone. Choosing the row from the claim sidestepped it.
///
/// The dilemma is gone — the audience is a projection now, and `OwnerViewUser` is simply
/// a different type from `PublicViewUser` — but the endpoint stays, because "who am I"
/// is still a question the server should answer from the token rather than one a client
/// should have to ask by id.
pub async fn me(
    AuthedJwt { user_id, .. }: AuthedJwt,
    State(state): State<AppState>,
) -> MyResult<impl IntoResponse> {
    // `None` while the user's own event is still in flight. The id comes from the claim
    // regardless, so this never has to 404.
    let profile = ViewUserRepository::find_owner_by_id(&state.db, user_id).await?;

    Ok(Json(Me {
        id: user_id,
        profile,
    }))
}

/// `GET /api/view/me/spots` — the caller's own listings, newest first.
///
/// Includes their inactive spots, which is what the live switch is for, and excludes
/// their deleted ones.
pub async fn spots(
    AuthedJwt { user_id, .. }: AuthedJwt,
    State(state): State<AppState>,
) -> MyResult<impl IntoResponse> {
    Ok(Json(
        ViewSpotRepository::find_all_by_owner_id(&state.db, user_id).await?,
    ))
}

/// `GET /api/view/me/bookings` — the caller's own bookings as a renter, newest first.
///
/// A booking the caller merely *hosts* is deliberately not here — that belongs on the
/// spot's manage page, where the host is already looking at their listing.
pub async fn bookings(
    AuthedJwt { user_id, .. }: AuthedJwt,
    State(state): State<AppState>,
) -> MyResult<impl IntoResponse> {
    Ok(Json(
        ViewBookingRepository::find_all_by_renter_id(&state.db, user_id).await?,
    ))
}

/// Query for [`wallet`]. Absent means the current month.
///
/// Optional rather than required so the first request needs to know nothing: a client
/// opening the wallet asks for `/me/wallet` and is told, in the response, which month it
/// got and which one to ask for next.
#[derive(Deserialize)]
pub struct WalletQuery {
    pub month: Option<String>,
}

/// `GET /api/view/me/wallet?month=YYYY-MM` — one month of everything that moved the
/// caller's money, newest first.
///
/// Charges as a host, charges as a renter, refunds either way, and withdrawals — four
/// sources in one ordered list, which is what a wallet is. `nextMonth` in the response
/// is the cursor: the next older month that holds anything, or null at the end.
///
/// **A month is a page.** Grouping by month is what the screen shows, so it is also how
/// the history is fetched — no offset to drift, and every page carries the totals for
/// exactly the rows in it.
pub async fn wallet(
    AuthedJwt { user_id, .. }: AuthedJwt,
    State(state): State<AppState>,
    Query(q): Query<WalletQuery>,
) -> MyResult<impl IntoResponse> {
    let now = Utc::now();
    let month = q.month.unwrap_or_else(|| policy::wallet::label(now));

    let (start, end) = policy::wallet::bounds(&month)
        .context_unprocessable_entity(("Invalid month", "Ask for a month as YYYY-MM."))?;

    let transactions =
        WalletRepository::find_month(&state.db, user_id, start, end, settled_before(now)).await?;
    let (in_cents, out_cents) = policy::wallet::totals(&transactions);

    Ok(Json(WalletMonth {
        month,
        in_cents,
        out_cents,
        next_month: WalletRepository::find_previous_month(&state.db, user_id, start).await?,
        transactions,
    }))
}

/// `GET /api/view/me/balance` — what the caller has earned, withdrawn and is waiting on.
///
/// This was `GET /api/payment/earnings`. It moved with every other read: payment-service
/// writes, view-service reads. What did **not** move is the figure a withdrawal actually
/// pays out — `PaymentService::request_payout` still computes that inside its own
/// transaction, under a lock, from its own tables, so nothing is ever spent against a
/// projection that may be a moment behind.
pub async fn balance(
    AuthedJwt { user_id, .. }: AuthedJwt,
    State(state): State<AppState>,
) -> MyResult<impl IntoResponse> {
    let mut conn = state.db.acquire().await?;

    Ok(Json(
        WalletRepository::balance(&mut conn, user_id, settled_before(Utc::now())).await?,
    ))
}

/// The instant a booking must have ended before for its payment to count as settled.
///
/// `SETTLEMENT_SECS` is read by **two** services and the two must agree: this decides
/// what a host is shown, and payment-service's copy decides what they can actually
/// withdraw. Different values mean a balance that offers more than the withdraw
/// endpoint will hand over, or less. `k8s/chart/values.yaml` sets both from one entry.
fn settled_before(now: DateTime<Utc>) -> DateTime<Utc> {
    now - chrono::Duration::seconds(crate::CONFIG.settlement_secs)
}
