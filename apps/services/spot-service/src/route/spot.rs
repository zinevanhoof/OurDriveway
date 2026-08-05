use axum::Json;
use axum::extract::{Multipart, State};
use axum::http::StatusCode;
use garde::Validate;
use serde::Serialize;
use shared::error::myerror::{ContextExt, MyError, MyResult};
use shared::events::STREAM_SPOTS;
use shared::extractors::authed_jwt::AuthedJwt;
use shared::requests::spot::CreateSpotRequest;
use tokio::{fs::File, io::AsyncWriteExt};

use crate::AppState;

#[derive(Serialize)]
pub struct CreatedResponse {
    pub id: String,
    /// `"SPOTS:4712"` — where this write landed in the log. The client echoes it
    /// back on its next read so a load balancer can't route it to an instance
    /// that hasn't projected this event yet.
    pub seq: String,
}

/// 202, not 201: the event is committed to the log, but the projections that
/// answer reads are still catching up. `seq` is how a caller waits for its own
/// write.
///
/// `Multipart` consumes the body, so it must stay the last extractor.
pub async fn create_spot(
    AuthedJwt { user_id, .. }: AuthedJwt,
    State(state): State<AppState>,
    multipart: Multipart,
) -> MyResult<(StatusCode, Json<CreatedResponse>)> {
    let (request, images) = parse_spot_form(multipart).await?;
    // Ownership comes from the verified token, never from the request body.
    let created = state
        .spot_service
        .create_spot(request, user_id, images)
        .await?;

    Ok((
        StatusCode::ACCEPTED,
        Json(CreatedResponse {
            id: shared::events::user::record_key(&created.spot_id),
            seq: format!("{STREAM_SPOTS}:{}", created.seq),
        }),
    ))
}

async fn parse_spot_form(mut multipart: Multipart) -> MyResult<(CreateSpotRequest, Vec<String>)> {
    let mut data = None;
    let mut images = vec![];

    while let Some(field) = multipart.next_field().await? {
        let name = field
            .name()
            .context_bad_request(("Bad Request", "unnamed field"))?
            .to_owned();

        match name.as_str() {
            "data" => {
                let json = field.text().await?;
                let request: CreateSpotRequest = serde_json::from_str(&json)
                    .context_bad_request(("Bad Request", "invalid data JSON"))?;
                request.validate()?; // garde -> MyError::Validation (422 { errors })
                data = Some(request);
            }
            "images" => {
                let file_name = field
                    .file_name()
                    .context_bad_request(("Bad Request", "image without filename"))?
                    .to_owned();

                let bytes = field.bytes().await?;

                // Origin-relative on purpose. These strings are persisted in an
                // event, so a hostname baked in here outlives the box it named:
                // every spot created on the old LAN IP would keep pointing at it
                // forever. Relative resolves against whatever origin served the
                // page — ingress, Caddy, or a Tauri webview.
                images.push(format!("/api/spot/uploads/{file_name}"));
                File::create(format!("uploads/{file_name}"))
                    .await?
                    .write_all(&bytes)
                    .await?;
            }
            other => tracing::warn!("unknown multipart field: {other}"),
        }
    }

    // Images live outside CreateSpotRequest, so garde can't reach them.
    if images.is_empty() {
        return Err(MyError::api(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Validation failed",
            "Add at least one photo.",
        ));
    }

    Ok((
        data.context_bad_request(("Bad Request", "missing data field"))?, // was silently ignored
        images,
    ))
}
