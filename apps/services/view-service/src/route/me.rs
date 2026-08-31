//! Everything the caller reads about themselves.
//!
//! Four routes with no parameters at all: the subject is the verified claim, so there
//! is nothing to extract and nothing to compare. `SPOTS_OWNED`, `BOOKINGS_RENTED` and
//! `PAYOUTS` each passed the reader's own id as a variable, which was safe only because
//! a table permission clause independently refused everyone else's rows. With the
//! clauses gone an id in a query string would be a request rather than a claim, so the
//! path says `/me` and the handler takes the id from the token.

use axum::{Json, extract::State, response::IntoResponse};
use shared::{
    error::myerror::MyResult, extractors::authed_jwt::AuthedJwt, projections::user::Me,
};

use crate::{
    AppState,
    repository::{
        booking_repository::ViewBookingRepository, payout_repository::ViewPayoutRepository,
        spot_repository::ViewSpotRepository, user_repository::ViewUserRepository,
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

/// `GET /api/view/me/payouts` — the caller's own withdrawal history.
///
/// Payouts and nothing else about money. What a renter was charged and what a host has
/// available are payment-service's to answer; a second copy here would eventually
/// disagree with the balance shown next to the withdraw button.
pub async fn payouts(
    AuthedJwt { user_id, .. }: AuthedJwt,
    State(state): State<AppState>,
) -> MyResult<impl IntoResponse> {
    Ok(Json(
        ViewPayoutRepository::find_all_by_owner_id(&state.db, user_id).await?,
    ))
}
