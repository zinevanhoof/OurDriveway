use axum::{extract::State, http::StatusCode, response::IntoResponse};
use shared::responses::common::accepted;
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
/// 202 with the log position rather than 204: the login that follows reads
/// `email_verified` from this service's own projection, so the client has to have
/// something to wait on or it can be told to verify an address it just verified.
pub async fn verify(
    State(state): State<AppState>,
    Valid(req): Valid<VerifyEmailRequest>,
) -> MyResult<impl IntoResponse> {
    let token = state.user_service.verify_email(req).await?;
    Ok(accepted(token))
}

/// Re-sends the verification email.
///
/// Always 204, including for an address that has no account and one already
/// verified. See `UserService::resend_verification` — the alternative is telling
/// an anonymous caller which addresses are registered.
pub async fn resend(
    State(state): State<AppState>,
    Valid(req): Valid<ResendVerificationRequest>,
) -> MyResult<impl IntoResponse> {
    state.user_service.resend_verification(req).await?;
    Ok(StatusCode::NO_CONTENT)
}
