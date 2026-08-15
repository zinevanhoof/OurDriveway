use axum::{Json, extract::State};
use axum_extra::extract::{
    CookieJar,
    cookie::{Cookie, SameSite},
};
use cookie::time::Duration;
use shared::error::myerror::MyResult;
use shared::extract::Valid;
use shared::{requests::user::LoginRequest, responses::user::AuthResponse};

use crate::AppState;

pub async fn login(
    jar: CookieJar,
    State(state): State<AppState>,
    Valid(req): Valid<LoginRequest>,
) -> MyResult<(CookieJar, Json<AuthResponse>)> {
    let result = state.user_service.login(&req.email, &req.password).await?;

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
