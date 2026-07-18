use axum::{Json, extract::Query};
use serde::Deserialize;
use shared::{error::myerror::MyResult, extractors::authed_jwt::AuthedJwt};

use crate::service::locationiq::{self, ResolvedAddress};

#[derive(Deserialize)]
pub struct SuggestQuery {
    pub q: String,
}

/// Proxy for LocationIQ autocomplete — keeps the API key server-side.
pub async fn suggest(
    _: AuthedJwt,
    Query(params): Query<SuggestQuery>,
) -> MyResult<Json<Vec<ResolvedAddress>>> {
    Ok(Json(locationiq::autocomplete(&params.q).await?))
}
