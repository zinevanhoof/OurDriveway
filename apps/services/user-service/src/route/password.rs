use axum::extract::State;
use axum::http::{HeaderName, StatusCode};
use shared::responses::common::X_VERSION;
use shared::{
    error::myerror::MyResult,
    extract::Valid,
    requests::user::{ForgotPasswordRequest, ResetPasswordRequest},
};

use crate::AppState;

/// Asks for a reset link.
///
/// Always 204, including for an address with no account. See
/// `UserService::forgot_password` — the alternative is telling an anonymous
/// caller which addresses are registered.
pub async fn forgot(
    State(state): State<AppState>,
    Valid(req): Valid<ForgotPasswordRequest>,
) -> MyResult<StatusCode> {
    state.user_service.forgot_password(req).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Sets the new password, given the token from the link.
///
/// Unauthenticated for the same reason `email::verify` is: the token is the
/// credential, and the person clicking has no session precisely because they
/// cannot log in.
///
/// Unlike verification, the link itself is *not* prefetch-sensitive — the effect
/// needs a password the user types — but the split is the same anyway: the mail
/// links to a page, and the page POSTs this.
///
/// 202 with the version rather than 204, as `email::verify` does and for the same
/// reason: the login that follows reads this service's own rows, so the client
/// needs something to wait on.
pub async fn reset(
    State(state): State<AppState>,
    Valid(req): Valid<ResetPasswordRequest>,
) -> MyResult<(StatusCode, [(HeaderName, String); 1])> {
    let version = state.user_service.reset_password(req).await?;
    Ok((StatusCode::ACCEPTED, [(X_VERSION, version)]))
}
