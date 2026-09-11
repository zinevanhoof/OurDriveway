use axum::{Json, extract::Query};
use shared::{
    error::myerror::MyResult, extractors::authed_jwt::AuthedJwt,
    requests::spot::AddressSuggestQuery, responses::spot::AddressSuggestResponse,
};

use crate::client::locationiq;

/// Proxy for LocationIQ autocomplete — keeps the API key server-side.
pub async fn suggest(
    _: AuthedJwt,
    Query(request): Query<AddressSuggestQuery>,
) -> MyResult<Json<Vec<AddressSuggestResponse>>> {
    Ok(Json(locationiq::autocomplete(&request.q).await?))
}
