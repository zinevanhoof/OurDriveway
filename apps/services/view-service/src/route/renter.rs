//! `renter_id = caller` — everything the caller reads as a renter.
//!
//! Their bookings, the next booking due, one booking by id, and a spot they booked.
//!
//! **The one exception to spots and bookings being separate reads is here:** a renter's
//! booking carries a small card of its spot, because every row of their list draws one.
//! See `RenterBookingProjection`. The full spot, with its host, is still its own route.
//!
//! The booking routes share one projection and do **not** share a response. `/next`
//! renders a card with a handful of fields and says so; sending it a full booking because
//! the query happened to select one is how a wire contract stops meaning anything.

use axum::{
    Json,
    extract::{Path, Query, State},
};
use shared::{
    error::myerror::MyResult,
    extractors::authed_jwt::AuthedJwt,
    responses::view::{
        NextBookingResponse, RenterBookingResponse, RenterBookingsPageResponse,
        RenterSpotResponse,
    },
};
use uuid::Uuid;

use crate::{AppState, route::BookingsQuery};

/// `GET /api/view/renter/bookings?scope=&status=&limit=&offset=` — one window of the
/// caller's own bookings, each with its spot card.
///
/// A booking the caller merely *hosts* is deliberately not here — that belongs under
/// `/host`, where the host is already looking at their listing.
pub async fn bookings(
    AuthedJwt { user_id, .. }: AuthedJwt,
    State(state): State<AppState>,
    Query(q): Query<BookingsQuery>,
) -> MyResult<Json<RenterBookingsPageResponse>> {
    Ok(Json(
        state
            .renter_service
            .bookings(user_id, q.scope, q.status, q.limit, q.offset)
            .await?,
    ))
}

/// `GET /api/view/renter/spots/{id}` — one spot the caller has booked.
///
/// The renter's own view of a listing, which unlike `/public/spots/{id}` still answers
/// after the host pauses or deletes it. 404 for a spot the caller never booked.
pub async fn spot(
    AuthedJwt { user_id, .. }: AuthedJwt,
    State(state): State<AppState>,
    Path(spot_id): Path<Uuid>,
) -> MyResult<Json<RenterSpotResponse>> {
    Ok(Json(state.renter_service.spot(spot_id, user_id).await?))
}

/// `GET /api/view/renter/bookings/next` — the home screen's next-up card.
///
/// `null` when there is nothing coming, rather than 404: "you have no bookings" is an
/// answer, and a 404 would make the home screen log an error on a perfectly ordinary
/// account.
pub async fn next(
    AuthedJwt { user_id, .. }: AuthedJwt,
    State(state): State<AppState>,
) -> MyResult<Json<Option<NextBookingResponse>>> {
    Ok(Json(state.renter_service.next(user_id).await?))
}

/// `GET /api/view/renter/bookings/{id}` — one of the caller's own bookings.
///
/// `renter_id = caller` and nothing else. This used to be
/// `(renter_id = $2 OR host_id = $2)`, one route answering both parties to a booking;
/// the host half is `/host/spots/{id}/bookings` now, which is where a host is already
/// looking.
///
/// 404 for a booking that is not the caller's, same as everywhere else.
pub async fn booking(
    AuthedJwt { user_id, .. }: AuthedJwt,
    State(state): State<AppState>,
    Path(booking_id): Path<Uuid>,
) -> MyResult<Json<RenterBookingResponse>> {
    Ok(Json(
        state.renter_service.booking(booking_id, user_id).await?,
    ))
}
