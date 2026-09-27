use axum::{
    extract::{Path, State},
    http::{HeaderName, StatusCode},
};
use uuid::Uuid;
use shared::extractors::authed_jwt::AuthedJwt;
use shared::responses::common::X_VERSION;
use shared::{error::myerror::MyResult, extract::Valid, requests::user::UpdateUserRequest};

use crate::AppState;

/// `PATCH /api/user` — the caller's own record, whichever half of it they are
/// writing. The client calls one of its screens the profile form; nothing back
/// here does, and neither does anything else in this workspace — that word is the
/// name of a screen, not of an entity. It edits the user row, there is
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

/// `POST /api/user/notifications/seen` — the caller opened their notifications.
pub async fn notifications_seen(
    AuthedJwt { user_id, .. }: AuthedJwt,
    State(state): State<AppState>,
) -> MyResult<(StatusCode, [(HeaderName, String); 1])> {
    let version = state.user_service.notifications_seen(&user_id).await?;
    Ok((StatusCode::ACCEPTED, [(X_VERSION, version)]))
}

/// `POST /api/user/notifications/{kind}/{subject_id}/dismiss` — the caller is done with
/// one notification. Only theirs: view-service scopes it to the caller's id.
pub async fn dismiss_notification(
    AuthedJwt { user_id, .. }: AuthedJwt,
    State(state): State<AppState>,
    Path((kind, subject_id)): Path<(String, Uuid)>,
) -> MyResult<(StatusCode, [(HeaderName, String); 1])> {
    let version = state
        .user_service
        .dismiss_notification(&user_id, kind, subject_id)
        .await?;
    Ok((StatusCode::ACCEPTED, [(X_VERSION, version)]))
}
