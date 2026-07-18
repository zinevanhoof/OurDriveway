use axum::extract::State;
use shared::{error::myerror::MyResult, extract::Valid, requests::user::SignupRequest};

use crate::AppState;

pub async fn signup(
    State(state): State<AppState>,
    Valid(req): Valid<SignupRequest>,
) -> MyResult<()> {
    state.user_service.signup(&req.email, &req.password).await?;

    Ok(())
}
