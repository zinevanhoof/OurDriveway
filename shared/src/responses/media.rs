use serde::Serialize;

/// One presigned upload: where the bytes go, and what to send back afterwards.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadUrlResponse {
    /// What the client sends back in the create/edit request, and what ends up in
    /// the event: the whole URL, already loadable. See [`crate::media`].
    pub url: String,
    /// Where to PUT the bytes. Good for one object, one method and one size.
    pub upload_url: String,
}
