//! No relationship required — what any signed-in caller may read about a listing.
//!
//! "Public" here means *no predicate on the caller*, not unauthenticated: both routes sit
//! behind `AuthedJwt` like every other. The caller's id appears exactly once, in
//! [`nearby`], and it is an **exclusion** rather than a scope — a host is not offered
//! their own driveway as somewhere to park.

use axum::{
    Json,
    extract::{Path, Query, State},
};
use serde::Deserialize;
use shared::{
    error::myerror::MyResult,
    extractors::authed_jwt::AuthedJwt,
    responses::view::{NearbyResponse, PublicSpotResponse},
};
use uuid::Uuid;

use crate::AppState;

/// `GET /api/view/public/spots/{id}` — one active spot as a prospective renter sees it.
///
/// The bookings that come with it are the availability answer and nothing else: which
/// slots are taken and until when, with no renter, no amount and no hold expiry. A host
/// looking at their own listing wants `/host/spots/{id}` instead.
///
/// The caller is authenticated but not otherwise used: an inactive spot 404s for everyone
/// here, including its host, because "public spot" is the whole question this route
/// answers.
pub async fn spot(
    _: AuthedJwt,
    State(state): State<AppState>,
    Path(spot_id): Path<Uuid>,
) -> MyResult<Json<PublicSpotResponse>> {
    Ok(Json(state.public_service.spot(spot_id).await?))
}

/// Query for [`nearby`].
///
/// Three required fields, so axum rejects a partial combination with its own 422 before
/// the handler runs. This used to be three `Option`s policed by a three-arm match, only
/// because one route meant both "mine" and "near me" — `/host/spots` answers the first
/// now.
///
/// Required is all this can police. Whether the radius is *reasonable* is a rule about
/// what will be read, so it lives with the read — see [`PublicService::nearby`].
///
/// [`PublicService::nearby`]: crate::service::public_service::PublicService::nearby
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NearbyQuery {
    pub lng: f64,
    pub lat: f64,
    pub meters: f64,
}

/// `GET /api/view/public/spots/nearby?lng=&lat=&meters=` — the map.
///
/// **Always excludes the caller's own spots.** `SPOTS_NEARBY` asked for that explicitly
/// and `SPOTS_IN_RADIUS` did not, which meant the map offered a host their own driveway as
/// somewhere to park.
pub async fn nearby(
    AuthedJwt { user_id, .. }: AuthedJwt,
    State(state): State<AppState>,
    Query(q): Query<NearbyQuery>,
) -> MyResult<Json<Vec<NearbyResponse>>> {
    Ok(Json(
        state
            .public_service
            .nearby(user_id, q.lng, q.lat, q.meters)
            .await?,
    ))
}
