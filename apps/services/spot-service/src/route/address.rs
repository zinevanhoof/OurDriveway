use axum::{Json, extract::Query, response::IntoResponse};
use shared::{
    error::myerror::MyResult, extractors::authed_jwt::AuthedJwt,
    requests::spot::AddressSuggestQuery,
};

use crate::client::locationiq;

/// Proxy for LocationIQ autocomplete — keeps the API key server-side.
pub async fn suggest(
    _: AuthedJwt,
    Query(request): Query<AddressSuggestQuery>,
) -> MyResult<impl IntoResponse> {
    Ok(Json(locationiq::autocomplete(&request.q).await?))
}
