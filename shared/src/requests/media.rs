//! What a client asks media-service for before it uploads.
//!
//! No `garde` impl, which makes this the one request in here that is not behind
//! `Valid`. Both of its rules need something this crate does not have: the size cap is
//! `MAX_UPLOAD_BYTES`, and config is impurity, while the type allowlist *is* the
//! extension table that names the object being minted. Both stay in the handler, next
//! to the presign they constrain — see `media-service/src/route.rs`.

use serde::Deserialize;

use crate::media::{PREFIX_AVATARS, PREFIX_SPOTS};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadUrlRequest {
    pub kind: Kind,
    pub content_type: String,
    /// The exact size of the body the client is about to PUT. Declared up front
    /// because it is the only way a presigned PUT can be capped — see the handler.
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
    pub fn prefix(self) -> &'static str {
        match self {
            Kind::Spot => PREFIX_SPOTS,
            Kind::Avatar => PREFIX_AVATARS,
        }
    }
}
