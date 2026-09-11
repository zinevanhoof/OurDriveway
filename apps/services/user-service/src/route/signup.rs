use axum::extract::State;
use axum::http::{HeaderName, StatusCode};
use shared::responses::common::X_VERSION;
use shared::{error::myerror::MyResult, extract::Valid, requests::user::SignupRequest};

use crate::AppState;

/// 202 with the version, like every other write. Nothing reads the new user
/// here — the next step is a link in their inbox — but a second submit of the same
/// form does, and echoing this is what turns that into a 409 rather than a silent
/// success off the dedupe window.
pub async fn signup(
    State(state): State<AppState>,
    Valid(req): Valid<SignupRequest>,
) -> MyResult<(StatusCode, [(HeaderName, String); 1])> {
    let version = state.user_service.signup(req).await?;

    Ok((StatusCode::ACCEPTED, [(X_VERSION, version)]))
}
