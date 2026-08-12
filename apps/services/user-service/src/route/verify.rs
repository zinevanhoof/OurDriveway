use axum::{extract::State, http::StatusCode};
use shared::{
    error::myerror::MyResult,
    extract::Valid,
    requests::user::{ResendVerificationRequest, VerifyEmailRequest},
};

use crate::AppState;

/// Confirms an address from the token in a mailed link.
///
/// Unauthenticated by design — the token is the credential, and the person
/// clicking has no session yet precisely because login is what verification
/// gates.
///
/// The frontend POSTs this from `/verify`; the link itself is a plain GET to a
/// page. That split is not decoration: mail scanners prefetch links, so anything
/// with an effect has to sit behind the verb they don't use.
pub async fn verify_email(
    State(state): State<AppState>,
    Valid(req): Valid<VerifyEmailRequest>,
) -> MyResult<StatusCode> {
    state.user_service.verify_email(&req.token).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Re-sends the verification email.
///
/// Always 204, including for an address that has no account and one already
/// verified. See `UserService::resend_verification` — the alternative is telling
/// an anonymous caller which addresses are registered.
pub async fn resend_verification(
    State(state): State<AppState>,
    Valid(req): Valid<ResendVerificationRequest>,
) -> MyResult<StatusCode> {
    state.user_service.resend_verification(&req.email).await?;
    Ok(StatusCode::NO_CONTENT)
}
