use axum::extract::Multipart;
use garde::Validate;
use shared::error::myerror::{ContextExt, MyResult};
use shared::requests::spot::CreateSpotRequest;
use tokio::{fs::File, io::AsyncWriteExt};

use crate::extractors::db_authenticated::DbAuthenticated;

pub async fn test_spot(
    DbAuthenticated(spot_service): DbAuthenticated,
    multipart: Multipart,
) -> MyResult<()> {
    let form = parse_spot_form(multipart).await?;
    spot_service.create_spot(form.0, form.1).await?;

    Ok(())
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

                images.push(format!("http://192.168.50.29:3002/api/uploads/{file_name}"));
                File::create(format!("uploads/{file_name}"))
                    .await?
                    .write_all(&bytes)
                    .await?;
            }
            other => tracing::warn!("unknown multipart field: {other}"),
        }
    }

    Ok((
        data.context_bad_request(("Bad Request", "missing data field"))?, // was silently ignored
        images,
    ))
}
