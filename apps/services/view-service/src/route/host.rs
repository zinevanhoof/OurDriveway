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
    extract::{Path, State},
    response::IntoResponse,
};
use shared::{error::myerror::MyResult, extractors::authed_jwt::AuthedJwt};
use uuid::Uuid;

use crate::AppState;

/// `GET /api/view/host/spots` — the caller's own listings, newest first.
///
/// Includes their inactive spots, which is what the live switch is for, and excludes their
/// deleted ones.
pub async fn spots(
    AuthedJwt { user_id, .. }: AuthedJwt,
    State(state): State<AppState>,
) -> MyResult<impl IntoResponse> {
    Ok(Json(state.host_service.spots(user_id).await?))
}

/// `GET /api/view/host/spots/{id}` — one spot as its host sees it.
///
/// Serves both the manage screen and the edit form. They differ in which fields they
/// render, not in which they may read, so they are one route: the edit form needs the
/// bookings to stop a host removing a slot someone has taken, and the manage screen needs
/// the same rows with their renters attached.
///
/// A non-host gets 404, not 403 — a 403 would confirm the existence of a listing the
/// caller is not allowed to see.
pub async fn spot(
    AuthedJwt { user_id, .. }: AuthedJwt,
    State(state): State<AppState>,
    Path(spot_id): Path<Uuid>,
) -> MyResult<impl IntoResponse> {
    Ok(Json(state.host_service.spot(spot_id, user_id).await?))
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
) -> MyResult<impl IntoResponse> {
    Ok(Json(state.host_service.balance(user_id).await?))
}
