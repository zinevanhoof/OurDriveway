use axum::{Json, extract::State, response::IntoResponse};
use shared::{
    error::myerror::MyResult,
    extractors::authed_jwt::AuthedJwt,
    responses::view::{Me, Profile},
};

use crate::AppState;

/// Why an endpoint rather than a GraphQL query: one `FOR select` clause cannot be
/// both "only me" and "public". `WHERE id = $token.ID` would break spot-owner
/// names on the map; `WHERE true` means `users { id }` returns everyone. Letting
/// the server pick the row from a signature-verified claim sidesteps the choice —
/// no second table, no duplicated rows, and no token parsing in the browser.
pub async fn me(
    AuthedJwt { user_id, .. }: AuthedJwt,
    State(state): State<AppState>,
) -> MyResult<impl IntoResponse> {
    // `None` while the user's own event is still in flight. The id comes from the
    // claim regardless, so this never has to 404.
    let profile = state.users.find_by_id(user_id).await?.map(Profile::from);

    Ok(Json(Me {
        id: user_id,
        profile,
    }))
}
