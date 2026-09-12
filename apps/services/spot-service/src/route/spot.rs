use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderName, StatusCode};
use shared::error::myerror::MyResult;
use shared::extract::Valid;
use shared::extractors::authed_jwt::AuthedJwt;
use shared::requests::spot::{CreateSpotRequest, UpdateSpotRequest};
use shared::responses::common::{BackfilledResponse, X_VERSION};
use uuid::Uuid;

use crate::AppState;

/// 202, not 201: the event is committed to the log, but the projections that
/// answer reads are still catching up. The `X-Version` header is how a caller waits
/// for its own write.
///
/// Answers with the version and nothing else — the minted id is not returned; see
/// `SpotService::create_spot`.
///
/// Plain JSON. Photos are already in R2 by the time this is called — the browser
/// uploaded them against a presigned URL from media-service — so `images` carries
/// keys, not bytes, and garde can vet the whole request in one place.
pub async fn create_spot(
    AuthedJwt { user_id, .. }: AuthedJwt,
    State(state): State<AppState>,
    Valid(request): Valid<CreateSpotRequest>,
) -> MyResult<(StatusCode, [(HeaderName, String); 1])> {
    // Ownership comes from the verified token, never from the request body.
    let version = state.spot_service.create_spot(&user_id, request).await?;

    Ok((StatusCode::ACCEPTED, [(X_VERSION, version)]))
}

/// 202 for the same reason as create: the log has it, the projections haven't.
///
/// Also the live switch — there is no separate endpoint for it. Every field of
/// [`UpdateSpotRequest`] is optional, so the manage screen's toggle is this route
/// with a body of `{ "active": false }` and nothing else.
pub async fn update_spot(
    AuthedJwt { user_id, .. }: AuthedJwt,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Valid(request): Valid<UpdateSpotRequest>,
) -> MyResult<(StatusCode, [(HeaderName, String); 1])> {
    let version = state
        .spot_service
        .update_spot(&user_id, &id, request)
        .await?;

    Ok((StatusCode::ACCEPTED, [(X_VERSION, version)]))
}

pub async fn delete_spot(
    AuthedJwt { user_id, .. }: AuthedJwt,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> MyResult<(StatusCode, [(HeaderName, String); 1])> {
    let version = state.spot_service.delete_spot(&user_id, &id).await?;
    Ok((StatusCode::ACCEPTED, [(X_VERSION, version)]))
}

/// `POST /internal/backfill` — re-emit every spot, for rebuilding a consumer.
///
/// Off the ingress and unauthenticated by construction; see the same handler in
/// user-service for why that is the whole of the access control.
pub async fn backfill(State(state): State<AppState>) -> MyResult<Json<BackfilledResponse>> {
    Ok(Json(BackfilledResponse {
        events: state.spot_service.backfill().await?,
    }))
}
