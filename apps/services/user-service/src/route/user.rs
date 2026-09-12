use axum::{
    Json,
    extract::State,
    http::{HeaderName, StatusCode},
};
use shared::extractors::authed_jwt::AuthedJwt;
use shared::responses::common::{BackfilledResponse, X_VERSION};
use shared::{error::myerror::MyResult, extract::Valid, requests::user::UpdateUserRequest};

use crate::AppState;

/// `PATCH /api/user` — the caller's own record, whichever half of it they are
/// writing. The client calls one screen the profile form and the other the
/// change-password form; nothing back here does. It edits the user row, there is
/// no other entity involved, and which event that becomes is
/// [`UserService::update_user`]'s call.
pub async fn update_user(
    AuthedJwt { user_id, .. }: AuthedJwt,
    State(state): State<AppState>,
    Valid(req): Valid<UpdateUserRequest>,
) -> MyResult<(StatusCode, [(HeaderName, String); 1])> {
    let version = state.user_service.update_user(&user_id, req).await?;
    Ok((StatusCode::ACCEPTED, [(X_VERSION, version)]))
}

/// `POST /internal/backfill` — re-emit every user, for rebuilding a consumer.
///
/// **Not under `/api`, and that is what keeps it private.** The ingress routes
/// `/api/<service>` and sends everything else to the SPA, so nothing outside the
/// cluster can reach this path on this service at all — the same effect
/// notification-service gets from `api: false`, without taking the rest of the
/// routes off the ingress with it. It follows that there is no `AuthedJwt` here:
/// there is no user whose token would mean anything, and reachability is the
/// control.
///
/// Unauthenticated *inside* the cluster, though. Anything that can dial the pod can
/// trigger a backfill, which costs a burst of re-projected events and no data loss.
/// If that ever needs to be more than a comment, a shared secret header is the
/// cheapest next rung.
///
/// Synchronous, so the count in the response is the real one and a script can wait
/// on it. It walks whole tables — see the ponytail note in `UserRepository::all` for
/// when that stops being reasonable.
pub async fn backfill(State(state): State<AppState>) -> MyResult<Json<BackfilledResponse>> {
    Ok(Json(BackfilledResponse {
        events: state.user_service.backfill().await?,
    }))
}
