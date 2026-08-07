use axum::Json;
use axum::extract::{Multipart, Path, State};
use axum::http::StatusCode;
use garde::Validate;
use serde::{Deserialize, Serialize};
use serde::de::DeserializeOwned;
use shared::error::myerror::{ContextExt, MyError, MyResult};
use shared::events::STREAM_SPOTS;
use shared::extractors::authed_jwt::AuthedJwt;
use shared::requests::spot::{CreateSpotRequest, UPLOAD_PREFIX, UpdateSpotRequest};
use tokio::{fs::File, io::AsyncWriteExt};
use uuid::Uuid;

use crate::AppState;

#[derive(Serialize)]
pub struct CreatedResponse {
    pub id: String,
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
/// `Multipart` consumes the body, so it must stay the last extractor.
pub async fn create_spot(
    AuthedJwt { user_id, .. }: AuthedJwt,
    State(state): State<AppState>,
    multipart: Multipart,
) -> MyResult<(StatusCode, Json<CreatedResponse>)> {
    let (request, images) = parse_spot_form::<CreateSpotRequest>(multipart).await?;
    // Images live outside CreateSpotRequest, so garde can't reach them.
    if images.is_empty() {
        return Err(no_photo());
    }
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

/// 202 for the same reason as create: the log has it, the projections haven't.
pub async fn update_spot(
    AuthedJwt { user_id, .. }: AuthedJwt,
    State(state): State<AppState>,
    Path(id): Path<String>,
    multipart: Multipart,
) -> MyResult<(StatusCode, Json<AcceptedResponse>)> {
    // Images the host kept arrive as URLs inside `data`; ones they just picked
    // arrive as file parts. Order is the client's — kept first, new appended.
    let (request, uploaded) = parse_spot_form::<UpdateSpotRequest>(multipart).await?;
    let images = [request.images.clone(), uploaded].concat();
    if images.is_empty() {
        return Err(no_photo());
    }

    let seq = state
        .spot_service
        .update_spot(&id, request, user_id, images)
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
    Path(id): Path<String>,
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
    Path(id): Path<String>,
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

/// The extension of a client-supplied filename, if it looks like one.
///
/// Anything else becomes `bin`: this ends up in a path, so the answer has to be
/// drawn from a set we control rather than trimmed out of their string.
fn extension(file_name: &str) -> &str {
    file_name
        .rsplit_once('.')
        .map(|(_, ext)| ext)
        .filter(|ext| {
            (1..=5).contains(&ext.len()) && ext.chars().all(|c| c.is_ascii_alphanumeric())
        })
        .unwrap_or("bin")
}

fn no_photo() -> MyError {
    MyError::api(
        StatusCode::UNPROCESSABLE_ENTITY,
        "Validation failed",
        "Add at least one photo.",
    )
}

/// Generic over the request type so create and update share one multipart parse
/// and one upload path — the two differ only in which fields the JSON carries.
async fn parse_spot_form<T: DeserializeOwned + Validate<Context = ()>>(
    mut multipart: Multipart,
) -> MyResult<(T, Vec<String>)> {
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
                let request: T = serde_json::from_str(&json)
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

                // The stored name is ours, never the client's. Theirs goes into a
                // filesystem path, so "../../etc/x" would write outside `uploads/` —
                // and two phones both sending IMG_1234.jpg would silently overwrite
                // each other's photo. Only the extension is worth keeping, and only
                // once it's been checked for being an extension.
                let stored = format!("{}.{}", Uuid::now_v7().simple(), extension(&file_name));

                // Origin-relative on purpose. These strings are persisted in an
                // event, so a hostname baked in here outlives the box it named:
                // every spot created on the old LAN IP would keep pointing at it
                // forever. Relative resolves against whatever origin served the
                // page — ingress, Caddy, or a Tauri webview.
                images.push(format!("{UPLOAD_PREFIX}{stored}"));
                File::create(format!("uploads/{stored}"))
                    .await?
                    .write_all(&bytes)
                    .await?;
            }
            other => tracing::warn!("unknown multipart field: {other}"),
        }
    }

    // The "at least one photo" rule is the caller's: on an edit, zero *uploaded*
    // files is the normal case — the host kept the ones already there.
    Ok((
        data.context_bad_request(("Bad Request", "missing data field"))?, // was silently ignored
        images,
    ))
}
