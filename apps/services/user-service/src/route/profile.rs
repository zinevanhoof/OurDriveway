use axum::{Json, extract::State, http::StatusCode};
use serde::Serialize;
use shared::events::STREAM_USERS;
use shared::extractors::authed_jwt::AuthedJwt;
use shared::{
    error::myerror::MyResult,
    extract::Valid,
    requests::user::{ChangePasswordRequest, UpdateProfileRequest},
};

use crate::AppState;

/// 202, not 200: the event is committed to the log, but the projections that
/// answer reads — this service's *and* view-service's — are still catching up.
/// `seq` is how the caller waits for its own write on the next read.
#[derive(Serialize)]
pub struct AcceptedResponse {
    pub seq: String,
}

pub async fn update_profile(
    AuthedJwt { user_id, .. }: AuthedJwt,
    State(state): State<AppState>,
    Valid(req): Valid<UpdateProfileRequest>,
) -> MyResult<(StatusCode, Json<AcceptedResponse>)> {
    let seq = state.user_service.update_profile(&uid(&user_id), req).await?;
    Ok(accepted(seq))
}

pub async fn change_password(
    AuthedJwt { user_id, .. }: AuthedJwt,
    State(state): State<AppState>,
    Valid(req): Valid<ChangePasswordRequest>,
) -> MyResult<(StatusCode, Json<AcceptedResponse>)> {
    let seq = state
        .user_service
        .change_password(&uid(&user_id), &req.current_password, &req.new_password)
        .await?;
    Ok(accepted(seq))
}

/// The claim is `"user:abc123"`; the repository addresses rows by the key alone.
fn uid(user_id: &str) -> String {
    user_id.strip_prefix("user:").unwrap_or(user_id).to_string()
}

fn accepted(seq: u64) -> (StatusCode, Json<AcceptedResponse>) {
    (
        StatusCode::ACCEPTED,
        Json(AcceptedResponse {
            seq: format!("{STREAM_USERS}:{seq}"),
        }),
    )
}
