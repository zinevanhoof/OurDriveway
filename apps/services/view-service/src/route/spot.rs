use axum::{
    Json,
    extract::{Path, Query, State},
    response::IntoResponse,
};
use chrono::Utc;
use serde::Deserialize;
use shared::{
    error::myerror::{ContextExt, MyResult},
    extractors::authed_jwt::AuthedJwt,
};
use uuid::Uuid;

use crate::{AppState, repository::spot_repository::ViewSpotRepository};

/// `GET /api/view/spots/:id` — one active spot as a prospective renter sees it.
///
/// The bookings that come with it are the availability answer and nothing else: which
/// slots are taken and until when, with no renter, no amount and no hold expiry. A host
/// looking at their own listing wants [`manage`] instead.
///
/// The caller is authenticated but not otherwise used — an inactive spot 404s for
/// everyone here, including its owner, because "public spot" is the whole question this
/// route answers.
pub async fn public(
    _: AuthedJwt,
    State(state): State<AppState>,
    Path(spot_id): Path<Uuid>,
) -> MyResult<impl IntoResponse> {
    // `Utc::now()` rather than a client-supplied `$now`. Three of the four GraphQL
    // documents this replaces made the browser pass one, which meant a client could ask
    // what was booked at any time it liked — harmless, but there is no reason to take
    // the instant from the caller when the server has one.
    let spot = ViewSpotRepository::find_public_by_id(&state.db, spot_id, Utc::now())
        .await?
        // 404 covers both "no such spot" and "not visible to you" — a 403 would confirm
        // the existence of a listing the caller is not allowed to see.
        .context_not_found(("Not Found", "That spot doesn't exist."))?;

    Ok(Json(spot))
}

/// `GET /api/view/spots/:id/manage` — one spot as its host sees it.
///
/// Serves both the manage screen and the edit form. They differ in which fields they
/// render, not in which they may read, so they are one route: the edit form needs the
/// bookings to stop a host removing a slot someone has taken, and the manage screen
/// needs the same rows with their renters attached.
///
/// A non-owner gets 404, not 403 — same reason as everywhere else.
pub async fn manage(
    AuthedJwt { user_id, .. }: AuthedJwt,
    State(state): State<AppState>,
    Path(spot_id): Path<Uuid>,
) -> MyResult<impl IntoResponse> {
    let spot = ViewSpotRepository::find_owner_by_id(&state.db, spot_id, user_id, Utc::now())
        .await?
        .context_not_found(("Not Found", "That spot doesn't exist."))?;

    Ok(Json(spot))
}

/// Query for [`nearby`].
///
/// Three required fields, so axum rejects a partial combination with its own 422 before
/// the handler runs. This used to be three `Option`s policed by a three-arm match, only
/// because one route meant both "mine" and "near me" — `/me/spots` answers the first now.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NearbyQuery {
    pub lng: f64,
    pub lat: f64,
    pub meters: f64,
}

/// `GET /api/view/spots/nearby?lng=&lat=&meters=` — the map.
///
/// **Always excludes the caller's own spots.** `SPOTS_NEARBY` asked for that explicitly
/// and `SPOTS_IN_RADIUS` did not, which meant the map offered a host their own driveway
/// as somewhere to park. It is unconditional in the query rather than a flag here.
pub async fn nearby(
    AuthedJwt { user_id, .. }: AuthedJwt,
    State(state): State<AppState>,
    Query(q): Query<NearbyQuery>,
) -> MyResult<impl IntoResponse> {
    // Bounded here rather than trusted: an unbounded radius turns the bbox into the
    // whole world and the index scan into a table scan. A value check, not a branch on
    // what the caller asked for.
    (q.meters > 0.0 && q.meters <= MAX_RADIUS_M).context_unprocessable_entity((
        "Invalid radius",
        "Search radius must be between 0 and 50km.",
    ))?;

    let spots =
        ViewSpotRepository::find_all_by_radius(&state.db, user_id, q.lng, q.lat, q.meters).await?;

    Ok(Json(spots))
}

/// Ceiling on a radius search, in metres.
///
/// The map's viewport is bounded, so this is not a limit any real client reaches — it
/// is what stops `?meters=40000000` from asking for the planet.
const MAX_RADIUS_M: f64 = 50_000.0;
