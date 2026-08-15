use axum::{Json, extract::State};
use serde::Serialize;
use shared::{error::myerror::MyResult, extractors::authed_jwt::AuthedJwt};
use surrealdb::types::SurrealValue;
use uuid::Uuid;

use crate::AppState;

#[derive(Serialize, SurrealValue)]
pub struct Profile {
    pub first_name: String,
    pub last_name: String,
    pub profile_picture: Option<String>,
}

#[derive(Serialize)]
pub struct Me {
    /// The plain uuid, straight from the verified claim, so it always resolves.
    ///
    /// Hyphenated, which is what the GraphQL `uuid` scalar takes on a filter
    /// (`owner_id: { eq: … }`). A `user(id:)` *lookup* needs it wrapped as
    /// `u'<uuid>'` instead — the frontend's `recordId()` does that.
    pub id: Uuid,
    /// `None` only in the moment between registering and the projection catching
    /// up. Nullable by design: the id is what callers actually need, and making
    /// this a 404 would turn a millisecond of lag into a broken sign-up flow.
    pub profile: Option<Profile>,
}

/// Why an endpoint rather than a GraphQL query: one `FOR select` clause cannot be
/// both "only me" and "public". `WHERE id = $token.ID` would break spot-owner
/// names on the map; `WHERE true` means `users { id }` returns everyone. Letting
/// the server pick the row from a signature-verified claim sidesteps the choice —
/// no second table, no duplicated rows, and no token parsing in the browser.
pub async fn me(
    AuthedJwt { user_id, .. }: AuthedJwt,
    State(state): State<AppState>,
) -> MyResult<Json<Me>> {
    let profile = state.repository.profile(&user_id).await?;
    Ok(Json(Me {
        id: user_id,
        profile,
    }))
}
