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
use serde::Deserialize;
use shared::{
    error::myerror::MyResult,
    extractors::authed_jwt::AuthedJwt,
    responses::view::{
        BalanceResponse, HostBookingsPageResponse, HostSpotListItemResponse, HostSpotResponse,
    },
};
use uuid::Uuid;

use crate::AppState;

/// `GET /api/view/host/spots` — the caller's own listings, newest first.
///
/// Includes their inactive spots, which is what the live switch is for, and excludes their
/// deleted ones.
pub async fn spots(
    AuthedJwt { user_id, .. }: AuthedJwt,
    State(state): State<AppState>,
) -> MyResult<Json<Vec<HostSpotListItemResponse>>> {
    Ok(Json(state.host_service.spots(user_id).await?))
}

/// `GET /api/view/host/spots/{id}` — one spot as its host sees it.
///
/// Serves both the manage screen and the edit form. They differ in which fields they
/// render, not in which they may read, so they are one route. It carries the taken slots
/// as one `booked` map — the edit form needs them to stop a host removing a slot someone
/// has taken — but not the booking rows, which are [`bookings`].
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

/// Query for [`bookings`]. All absent means the first twenty of every booking still to
/// come.
///
/// Plain options for the same reason as [`WalletQuery`]: each is a rule stated once next
/// to the read, and a 422 for any of them comes from the service rather than from an
/// extractor that would answer before the caller's ownership of the spot had been
/// established.
///
/// [`WalletQuery`]: crate::route::account::WalletQuery
#[derive(Deserialize)]
pub struct BookingsQuery {
    /// `upcoming` (the default) or `past`.
    pub scope: Option<String>,
    /// Comma-separated, e.g. `confirmed` or `cancelled,released`. Absent is every status.
    pub status: Option<String>,
    /// 1 to 50, 20 when absent.
    pub limit: Option<i64>,
    /// 0 when absent. The client never computes one — it asks for what `nextOffset` said.
    pub offset: Option<i64>,
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
