use axum::{Json, extract::State, response::IntoResponse};
use axum_extra::extract::CookieJar;
use shared::error::myerror::{ContextExt, MyResult};
use shared::responses::user::AuthResponse;
use uuid::Uuid;

use crate::AppState;

pub async fn refresh(jar: CookieJar, State(state): State<AppState>) -> MyResult<impl IntoResponse> {
    let refresh_token = jar
        .get("refresh-token")
        .map(|c| c.value())
        .context_bad_request(("Bad Request", "Missing refresh token"))?;

    let (jwt, new_refresh_token, token) = state
        .refresh_token_service
        .rotate(
            Uuid::parse_str(refresh_token)
                .context_bad_request(("Bad Request", "Malformed refresh token"))?,
        )
        .await?;

    let jar = jar.add(crate::auth::cookie::set(new_refresh_token));

    Ok((
        jar,
        Json(AuthResponse::new(jwt, token)),
    ))
}
