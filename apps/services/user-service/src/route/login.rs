use axum::{Json, extract::State, http::HeaderName};
use axum_extra::extract::CookieJar;
use shared::error::myerror::MyResult;
use shared::extract::Valid;
use shared::{
    requests::user::LoginRequest,
    responses::{common::X_VERSION, user::LoginResponse},
};

use crate::AppState;

pub async fn login(
    jar: CookieJar,
    State(state): State<AppState>,
    Valid(req): Valid<LoginRequest>,
) -> MyResult<(CookieJar, [(HeaderName, String); 1], Json<LoginResponse>)> {
    // Two services, in order: one says the credentials are good and hands back the
    // user, the other decides what session to open for them.
    let user = state.user_service.authenticate(req).await?;
    let (jwt, refresh_token, version) = state.refresh_token_service.issue(&user).await?;

    let jar = jar.add(crate::auth::cookie::set(refresh_token));

    // 200 with the version in a header, not 202: the session exists and the token is
    // usable now. The header is what makes the *next* request wait for the SESSIONS
    // projection, which is why login carries one at all.
    Ok((jar, [(X_VERSION, version)], Json(LoginResponse { access_token: jwt })))
}
