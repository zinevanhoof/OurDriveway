use axum::{Json, extract::State};
use axum_extra::extract::{
    CookieJar,
    cookie::{Cookie, SameSite},
};
use cookie::time::Duration;
use shared::error::myerror::{ContextExt, MyResult};
use shared::responses::user::AuthResponse;
use uuid::Uuid;

use crate::AppState;

pub async fn refresh(
    jar: CookieJar,
    State(state): State<AppState>,
) -> MyResult<(CookieJar, Json<AuthResponse>)> {
    let refresh_token = jar
        .get("refresh-token")
        .map(|c| c.value())
        .context_bad_request(("Bad Request", "Missing refresh token"))?;

    let result = state
        .user_service
        .refresh(
            Uuid::parse_str(refresh_token)
                .context_bad_request(("Bad Request", "Malformed refresh token"))?,
        )
        .await?;

    let jar = jar.add(
        Cookie::build(("refresh-token", result.1))
            .http_only(true)
            .same_site(SameSite::Strict)
            .path("/api/user/refresh")
            .max_age(Duration::days(30))
            .build(),
    );

    Ok((jar, Json(AuthResponse::new(result.0))))
}
