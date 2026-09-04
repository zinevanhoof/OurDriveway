use axum::{Json, extract::State, response::IntoResponse};
use shared::{
    error::myerror::MyResult,
    extractors::authed_jwt::AuthedJwt,
    responses::payment::{AccountSessionResponse, ConnectStatusResponse},
};

use crate::{AppState, service::connect_service::ConnectStatus};

/// Whether the caller can be paid, and where.
///
/// The withdraw screen's one gate, and the reason that screen needs nothing else: it is
/// handed a maximum in its URL and asks this for everything else.
///
/// Scoped to the caller by construction — there is no path parameter and no way to ask
/// about anyone else's account. 200 rather than 202: this reads Stripe, not a
/// projection, so there is no version to wait for.
pub async fn status(
    AuthedJwt { user_id, .. }: AuthedJwt,
    State(state): State<AppState>,
) -> MyResult<impl IntoResponse> {
    let (state, bank_last4) = match state.connect_service.status(&user_id).await? {
        ConnectStatus::NeedsCountry => ("needs_country", None),
        ConnectStatus::None => ("none", None),
        ConnectStatus::Onboarding => ("onboarding", None),
        ConnectStatus::Enabled { bank_last4 } => ("enabled", bank_last4),
    };

    Ok(Json(ConnectStatusResponse { state, bank_last4 }))
}

/// The client secret for Connect's embedded components, creating the caller's connected
/// account if this is their first time.
///
/// POST because it is not a read: the first call creates an account at Stripe, and every
/// call creates a session object. The screen calls it on mount and again whenever the
/// component asks for a fresh secret — see `fetchClientSecret` in `lib/connect.ts`.
pub async fn account_session(
    AuthedJwt { user_id, .. }: AuthedJwt,
    State(state): State<AppState>,
) -> MyResult<impl IntoResponse> {
    Ok(Json(AccountSessionResponse {
        client_secret: state.connect_service.account_session(&user_id).await?,
    }))
}
