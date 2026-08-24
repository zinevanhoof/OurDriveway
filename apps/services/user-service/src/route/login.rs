use axum::{Json, extract::State, response::IntoResponse};
use axum_extra::extract::CookieJar;
use bus::format_seq;
use shared::error::myerror::MyResult;
use shared::events::STREAM_SESSIONS;
use shared::extract::Valid;
use shared::{requests::user::LoginRequest, responses::user::AuthResponse};

use crate::AppState;

pub async fn login(
    jar: CookieJar,
    State(state): State<AppState>,
    Valid(req): Valid<LoginRequest>,
) -> MyResult<impl IntoResponse> {
    // Two services, in order: one says the credentials are good and hands back the
    // user, the other decides what session to open for them.
    let user = state.user_service.authenticate(req).await?;
    let (jwt, refresh_token, seq) = state.refresh_token_service.issue(&user).await?;

    let jar = jar.add(crate::auth::cookie::set(refresh_token));

    Ok((
        jar,
        Json(AuthResponse::new(jwt, format_seq(STREAM_SESSIONS, seq))),
    ))
}
