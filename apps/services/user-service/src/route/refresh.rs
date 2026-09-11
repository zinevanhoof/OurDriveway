use axum::{Json, extract::State, http::HeaderName};
use axum_extra::extract::CookieJar;
use shared::error::myerror::{ContextExt, MyResult};
use shared::responses::{common::X_VERSION, user::RefreshResponse};
use uuid::Uuid;

use crate::AppState;

pub async fn refresh(
    jar: CookieJar,
    State(state): State<AppState>,
) -> MyResult<(CookieJar, [(HeaderName, String); 1], Json<RefreshResponse>)> {
    let refresh_token = jar
        .get("refresh-token")
        .map(|c| c.value())
        .context_bad_request(("Bad Request", "Missing refresh token"))?;

    let (jwt, new_refresh_token, version) = state
        .refresh_token_service
        .rotate(
            Uuid::parse_str(refresh_token)
                .context_bad_request(("Bad Request", "Malformed refresh token"))?,
        )
        .await?;

    let jar = jar.add(crate::auth::cookie::set(new_refresh_token));

    Ok((jar, [(X_VERSION, version)], Json(RefreshResponse { access_token: jwt })))
}
