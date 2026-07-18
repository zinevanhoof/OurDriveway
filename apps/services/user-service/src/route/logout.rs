use axum::extract::State;
use shared::error::myerror::{ContextExt, MyResult};
use axum_extra::extract::{
    CookieJar,
    cookie::{Cookie, SameSite},
};
use uuid::Uuid;

use crate::AppState;

pub async fn logout(jar: CookieJar, State(state): State<AppState>) -> MyResult<CookieJar> {
    let refresh_token = jar
        .get("refresh-token")
        .map(|c| c.value())
        .context_bad_request(("Bad Request", "Missing refresh token"))?;

    state
        .user_service
        .logout(
            Uuid::parse_str(refresh_token)
                .context_bad_request(("Bad Request", "Malformed refresh token"))?,
        )
        .await?;

    let jar = jar.remove(
        Cookie::build("refresh-token")
            .http_only(true)
            .same_site(SameSite::Strict)
            .path("/api/user/refresh")
            .build(),
    );

    Ok(jar)
}
