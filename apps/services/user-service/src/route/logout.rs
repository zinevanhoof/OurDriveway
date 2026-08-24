use axum::extract::State;
use axum::response::IntoResponse;
use axum_extra::extract::CookieJar;
use shared::error::myerror::{ContextExt, MyResult};
use uuid::Uuid;

use crate::AppState;

pub async fn logout(jar: CookieJar, State(state): State<AppState>) -> MyResult<impl IntoResponse> {
    let refresh_token = jar
        .get("refresh-token")
        .map(|c| c.value())
        .context_bad_request(("Bad Request", "Missing refresh token"))?;

    state
        .refresh_token_service
        .revoke(
            Uuid::parse_str(refresh_token)
                .context_bad_request(("Bad Request", "Malformed refresh token"))?,
        )
        .await?;

    let jar = jar.remove(crate::auth::cookie::clear());

    Ok(jar)
}
