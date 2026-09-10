//! `renter_id = caller` — everything the caller reads as a renter.
//!
//! Three routes over one projection and one join, which is why
//! `RenterBookingProjection` carries that join on itself via `HasQuery`. They differ in
//! `WHERE` and `LIMIT`, not in shape.
//!
//! They do **not** share a response. `/next` renders a card with four fields and says so;
//! sending it a full booking because the query happened to select one is how a wire
//! contract stops meaning anything.

use axum::{
    Json,
    extract::{Path, State},
    response::IntoResponse,
};
use shared::{error::myerror::MyResult, extractors::authed_jwt::AuthedJwt};
use uuid::Uuid;

use crate::AppState;

/// `GET /api/view/renter/bookings` — the caller's own bookings, newest first.
///
/// A booking the caller merely *hosts* is deliberately not here — that belongs on the
/// spot's page under `/host`, where the host is already looking at their listing.
pub async fn bookings(
    AuthedJwt { user_id, .. }: AuthedJwt,
    State(state): State<AppState>,
) -> MyResult<impl IntoResponse> {
    Ok(Json(state.renter_service.bookings(user_id).await?))
}

/// `GET /api/view/renter/bookings/next` — the home screen's next-up card.
///
/// `null` when there is nothing coming, rather than 404: "you have no bookings" is an
/// answer, and a 404 would make the home screen log an error on a perfectly ordinary
/// account.
pub async fn next(
    AuthedJwt { user_id, .. }: AuthedJwt,
    State(state): State<AppState>,
) -> MyResult<impl IntoResponse> {
    Ok(Json(state.renter_service.next(user_id).await?))
}

/// `GET /api/view/renter/bookings/{id}` — one of the caller's own bookings.
///
/// `renter_id = caller` and nothing else. This used to be
/// `(renter_id = $2 OR host_id = $2)`, one route answering both parties to a booking;
/// the host half is `/host/spots/{id}` now, which is where a host is already looking.
///
/// 404 for a booking that is not the caller's, same as everywhere else.
pub async fn booking(
    AuthedJwt { user_id, .. }: AuthedJwt,
    State(state): State<AppState>,
    Path(booking_id): Path<Uuid>,
) -> MyResult<impl IntoResponse> {
    Ok(Json(
        state.renter_service.booking(booking_id, user_id).await?,
    ))
}
