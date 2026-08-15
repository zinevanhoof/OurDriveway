use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use shared::error::myerror::MyResult;
use shared::events::STREAM_SPOTS;
use shared::extract::Valid;
use shared::extractors::authed_jwt::AuthedJwt;
use shared::requests::spot::{CreateSpotRequest, UpdateSpotRequest};
use uuid::Uuid;

use crate::AppState;

#[derive(Serialize)]
pub struct CreatedResponse {
    /// The uuid, hyphenated. The client wraps it as `u'<uuid>'` before handing it
    /// to a GraphQL `spot(id:)` lookup — see `recordId()` in the frontend.
    pub id: Uuid,
    /// `"SPOTS:4712"` — where this write landed in the log. The client echoes it
    /// back on its next read so a load balancer can't route it to an instance
    /// that hasn't projected this event yet.
    pub seq: String,
}

/// The same `seq`, for writes to a spot that already has an id.
#[derive(Serialize)]
pub struct AcceptedResponse {
    pub seq: String,
}

/// 202, not 201: the event is committed to the log, but the projections that
/// answer reads are still catching up. `seq` is how a caller waits for its own
/// write.
///
/// Plain JSON. Photos are already in R2 by the time this is called — the browser
/// uploaded them against a presigned URL from media-service — so `images` carries
/// keys, not bytes, and garde can vet the whole request in one place.
pub async fn create_spot(
    AuthedJwt { user_id, .. }: AuthedJwt,
    State(state): State<AppState>,
    Valid(request): Valid<CreateSpotRequest>,
) -> MyResult<(StatusCode, Json<CreatedResponse>)> {
    // Ownership comes from the verified token, never from the request body.
    let created = state.spot_service.create_spot(request, user_id).await?;

    Ok((
        StatusCode::ACCEPTED,
        Json(CreatedResponse {
            id: created.spot_id,
            seq: format!("{STREAM_SPOTS}:{}", created.seq),
        }),
    ))
}

/// 202 for the same reason as create: the log has it, the projections haven't.
pub async fn update_spot(
    AuthedJwt { user_id, .. }: AuthedJwt,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Valid(request): Valid<UpdateSpotRequest>,
) -> MyResult<(StatusCode, Json<AcceptedResponse>)> {
    let seq = state
        .spot_service
        .update_spot(&id, request, user_id)
        .await?;

    Ok(accepted(seq))
}

#[derive(Deserialize)]
pub struct ActiveRequest {
    pub active: bool,
}

/// The live switch. Its own route rather than a field on the edit form: it is one
/// tap from a screen that isn't editing anything, and it publishes a different event.
pub async fn set_active(
    AuthedJwt { user_id, .. }: AuthedJwt,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(request): Json<ActiveRequest>,
) -> MyResult<(StatusCode, Json<AcceptedResponse>)> {
    let seq = state
        .spot_service
        .set_active(&id, request.active, user_id)
        .await?;
    Ok(accepted(seq))
}

pub async fn delete_spot(
    AuthedJwt { user_id, .. }: AuthedJwt,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> MyResult<(StatusCode, Json<AcceptedResponse>)> {
    let seq = state.spot_service.delete_spot(&id, user_id).await?;
    Ok(accepted(seq))
}

fn accepted(seq: u64) -> (StatusCode, Json<AcceptedResponse>) {
    (
        StatusCode::ACCEPTED,
        Json(AcceptedResponse {
            seq: format!("{STREAM_SPOTS}:{seq}"),
        }),
    )
}
