use std::time::Duration;

use aws_sdk_s3::presigning::PresigningConfig;
use axum::{Json, extract::State, http::StatusCode};
use serde::{Deserialize, Serialize};
use shared::error::myerror::{MyError, MyResult};
use shared::extractors::authed_jwt::AuthedJwt;
use shared::media::{PREFIX_AVATARS, PREFIX_SPOTS};
use uuid::Uuid;

use crate::{AppState, CONFIG};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadUrlRequest {
    pub kind: Kind,
    pub content_type: String,
    /// The exact size of the body the client is about to PUT. Declared up front
    /// because it is the only way a presigned PUT can be capped — see below.
    pub content_length: u64,
}

/// Which part of the bucket the object belongs in. An enum rather than a free
/// string: it becomes a key prefix, and the set of prefixes is ours.
#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    Spot,
    Avatar,
}

impl Kind {
    fn prefix(self) -> &'static str {
        match self {
            Kind::Spot => PREFIX_SPOTS,
            Kind::Avatar => PREFIX_AVATARS,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadUrlResponse {
    /// What the client sends back in the create/edit request, and what ends up in
    /// the event. Bare, with no hostname — see `shared::media`.
    pub key: String,
    /// Where to PUT the bytes. Good for one object, one method and one size.
    pub upload_url: String,
}

/// Mints a presigned PUT so the browser can upload straight to R2.
///
/// Authenticated, because an open endpoint here is an open write to the bucket.
/// It is the only gate: the URL this returns is unauthenticated by design, so
/// everything that constrains the upload has to be baked into it at this point.
pub async fn upload_url(
    _: AuthedJwt,
    State(state): State<AppState>,
    Json(request): Json<UploadUrlRequest>,
) -> MyResult<Json<UploadUrlResponse>> {
    let extension = extension_for(&request.content_type).ok_or_else(|| {
        MyError::api(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Unsupported image type",
            "Photos must be JPEG, PNG or WebP.",
        )
    })?;

    if request.content_length == 0 || request.content_length > CONFIG.max_upload_bytes {
        return Err(MyError::api(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Image too large",
            format!(
                "Each photo must be under {} MB.",
                CONFIG.max_upload_bytes / (1024 * 1024)
            ),
        ));
    }

    // The key is ours, never the client's. It used to be derived from the uploaded
    // filename, which meant sniffing an extension out of a string that could also
    // contain "../"; deriving it from a content type we just checked against a
    // fixed list removes the parsing problem rather than guarding it.
    let key = format!(
        "{}/{}.{extension}",
        request.kind.prefix(),
        Uuid::now_v7().simple()
    );

    // `content_length` and `content_type` are set so they are SIGNED, not merely
    // suggested: a presigned PUT has no equivalent of a POST policy's
    // content-length-range, so binding the declared size into the signature is
    // what replaces the DefaultBodyLimit that used to live in spot-service. A
    // client that asks for 1 MB cannot reuse the URL to push 4 GB.
    let presigned = state
        .s3
        .put_object()
        .bucket(&CONFIG.s3_bucket)
        .key(&key)
        .content_type(&request.content_type)
        .content_length(request.content_length as i64)
        .presigned(
            PresigningConfig::expires_in(Duration::from_secs(CONFIG.presign_expiry_secs))
                .map_err(|e| internal("presign config", e))?,
        )
        .await
        .map_err(|e| internal("presign", e))?;

    Ok(Json(UploadUrlResponse {
        key,
        upload_url: presigned.uri().to_string(),
    }))
}

/// The stored extension for an accepted content type, or `None` if we don't take
/// that type at all.
///
/// The allowlist is the point — this is what the client is allowed to put in the
/// bucket, and the extension is a consequence of it rather than a second input.
fn extension_for(content_type: &str) -> Option<&'static str> {
    match content_type {
        "image/jpeg" => Some("jpeg"),
        "image/png" => Some("png"),
        "image/webp" => Some("webp"),
        _ => None,
    }
}

/// The client learns nothing beyond "we broke"; the log gets the real cause.
fn internal(what: &str, error: impl std::fmt::Display) -> MyError {
    tracing::error!("{what} failed: {error}");
    MyError::api(
        StatusCode::INTERNAL_SERVER_ERROR,
        "Upload unavailable",
        "Could not prepare the upload. Try again.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use shared::media::is_media_key;

    /// The allowlist and the extension table are the same decision, so they can't
    /// disagree — but the key built from one has to satisfy the validator that
    /// spot-service and user-service run on the way back in.
    #[test]
    fn minted_keys_pass_the_validator_that_guards_the_event_log() {
        for (content_type, kind, prefix) in [
            ("image/jpeg", Kind::Spot, PREFIX_SPOTS),
            ("image/png", Kind::Spot, PREFIX_SPOTS),
            ("image/webp", Kind::Avatar, PREFIX_AVATARS),
        ] {
            let extension = extension_for(content_type).expect("allowlisted");
            let key = format!("{}/{}.{extension}", kind.prefix(), Uuid::now_v7().simple());
            assert!(is_media_key(&key, prefix), "{key} rejected");
        }
    }

    /// The size cap is only real if the SDK actually SIGNS content-length.
    ///
    /// Nothing in the type system says it does — `.content_length()` could equally
    /// be serialised as a plain header the client is free to ignore, in which case
    /// the URL would accept a body of any size and `MAX_UPLOAD_BYTES` would be
    /// decoration. This asserts the header is inside `X-Amz-SignedHeaders`, so an
    /// SDK upgrade that changes it fails here instead of silently un-capping
    /// uploads.
    ///
    /// Presigning is a local computation, so this needs no network and no bucket.
    #[tokio::test]
    async fn the_size_cap_is_signed_into_the_url() {
        let client = aws_sdk_s3::Client::from_conf(
            aws_sdk_s3::Config::builder()
                .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
                .region(aws_sdk_s3::config::Region::new("auto"))
                .endpoint_url("https://example.r2.cloudflarestorage.com")
                .force_path_style(true)
                .credentials_provider(aws_sdk_s3::config::Credentials::new(
                    "key", "secret", None, None, "test",
                ))
                .build(),
        );

        let presigned = client
            .put_object()
            .bucket("bucket")
            .key("spots/019fd9a1a3cb7d12b96249db33e2a909.jpeg")
            .content_type("image/jpeg")
            .content_length(1234)
            .presigned(PresigningConfig::expires_in(Duration::from_secs(900)).unwrap())
            .await
            .unwrap();

        let uri = presigned.uri().to_string();
        let signed = uri
            .split("X-Amz-SignedHeaders=")
            .nth(1)
            .and_then(|rest| rest.split('&').next())
            .expect("presigned URL always carries X-Amz-SignedHeaders");

        assert!(
            signed.contains("content-length"),
            "content-length is not signed, so MAX_UPLOAD_BYTES is unenforceable: {signed}"
        );
        assert!(signed.contains("content-type"), "signed headers: {signed}");
    }

    #[test]
    fn rejects_types_that_are_not_images_we_serve() {
        for bad in [
            "text/html",
            "image/svg+xml", // renders script when served from a public bucket
            "application/octet-stream",
            "image/jpeg; charset=utf-8", // exact match only
            "",
        ] {
            assert!(extension_for(bad).is_none(), "accepted {bad:?}");
        }
    }
}
