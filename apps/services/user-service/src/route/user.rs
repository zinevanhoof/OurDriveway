use axum::{extract::State, response::IntoResponse};
use bus::format_seq;
use shared::events::STREAM_USERS;
use shared::extractors::authed_jwt::AuthedJwt;
use shared::responses::common::accepted;
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
) -> MyResult<impl IntoResponse> {
    let seq = state.user_service.update_user(&user_id, req).await?;
    Ok(accepted(format_seq(STREAM_USERS, seq)))
}
