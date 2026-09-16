//! `host_id = caller` — everything the caller reads as a host.
//!
//! One predicate for the whole namespace, and it is on the *spot*: a host's listings, one
//! of their listings, and the money those listings earned. A booking is never reached by
//! id here — it is a child of a spot whose ownership the parent statement already proved.
//!
//! `/spots/{id}/manage` is gone, and with it a path segment shared with nothing. The
//! screen it served is `GET /host/spots/{id}`, which says who may read it in the same
//! place it says what it returns.

use axum::{
    Json,
    extract::{Path, Query, State},
};
use shared::{
    error::myerror::MyResult,
    extractors::authed_jwt::AuthedJwt,
    general_models::booking::Booked,
    responses::view::{
        BalanceResponse, HostBookingsPageResponse, HostSpotResponse, HostSpotsPageResponse,
    },
};
use uuid::Uuid;

use crate::{
    AppState,
    route::{BookingsQuery, PageQuery},
};

/// `GET /api/view/host/spots?limit=&offset=` — one window of the caller's own listings,
/// newest first.
///
/// Includes their inactive spots, which is what the live switch is for, and excludes their
/// deleted ones.
pub async fn spots(
    AuthedJwt { user_id, .. }: AuthedJwt,
    State(state): State<AppState>,
    Query(q): Query<PageQuery>,
) -> MyResult<Json<HostSpotsPageResponse>> {
    Ok(Json(
        state.host_service.spots(user_id, q.limit, q.offset).await?,
    ))
}

/// `GET /api/view/host/spots/{id}` — one spot as its host sees it.
///
/// Serves both the manage screen and the edit form. They differ in which fields they
/// render, not in which they may read, so they are one route. No bookings: the rows are
/// [`bookings`] and the taken slots [`booked`].
///
/// A non-host gets 404, not 403 — a 403 would confirm the existence of a listing the
/// caller is not allowed to see.
pub async fn spot(
    AuthedJwt { user_id, .. }: AuthedJwt,
    State(state): State<AppState>,
    Path(spot_id): Path<Uuid>,
) -> MyResult<Json<HostSpotResponse>> {
    Ok(Json(state.host_service.spot(spot_id, user_id).await?))
}

/// `GET /api/view/host/spots/{id}/booked` — every slot still held on one spot, merged.
///
/// The edit form's warning before a host removes hours someone has taken. One map rather
/// than the booking rows: it is one question, and it has to see every future slot at once,
/// which the paged [`bookings`] would make it walk for.
pub async fn booked(
    AuthedJwt { user_id, .. }: AuthedJwt,
    State(state): State<AppState>,
    Path(spot_id): Path<Uuid>,
) -> MyResult<Json<Booked>> {
    Ok(Json(state.host_service.booked(spot_id, user_id).await?))
}

/// `GET /api/view/host/spots/{id}/bookings?scope=&status=&limit=&offset=` — one window
/// of one spot's bookings.
///
/// Both the manage screen's preview (`status=confirmed&limit=2`) and the paged screen
/// behind it. It is a separate route from [`spot`] rather than a parameter on it because
/// it is a separate read with a separate cache lifetime: a spot is edited rarely and its
/// bookings change under it constantly.
///
/// Same 404-not-403 as [`spot`], and from the same statement — the ownership check is
/// the first thing `spot_bookings` does.
pub async fn bookings(
    AuthedJwt { user_id, .. }: AuthedJwt,
    State(state): State<AppState>,
    Path(spot_id): Path<Uuid>,
    Query(q): Query<BookingsQuery>,
) -> MyResult<Json<HostBookingsPageResponse>> {
    Ok(Json(
        state
            .host_service
            .spot_bookings(spot_id, user_id, q.scope, q.status, q.limit, q.offset)
            .await?,
    ))
}

/// `GET /api/view/host/balance` — what the caller has earned, withdrawn and is waiting on.
///
/// This was `GET /api/payment/earnings`. It moved with every other read: payment-service
/// writes, view-service reads.
///
/// Under `host` and not `account` because it is money a *listing* earned. A renter who has
/// never hosted gets zeroes, which is the honest answer rather than a 404.
pub async fn balance(
    AuthedJwt { user_id, .. }: AuthedJwt,
    State(state): State<AppState>,
) -> MyResult<Json<BalanceResponse>> {
    Ok(Json(state.host_service.balance(user_id).await?))
}
