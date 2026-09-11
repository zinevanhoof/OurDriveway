use serde::Serialize;

// Login and refresh answer with the same one field and are still two types: a response
// is named for the request it answers, and these are two requests. They would share a
// shape by coincidence the day one of them needs a second field.
//
// The session's position in the log used to be a `seq` field on the shared type, for a
// reason that still holds: the other half of a session, the refresh token, goes into
// an httponly cookie the client cannot read, so without the position echoed back,
// `/refresh` could legitimately read the projection before the session it is rotating
// has landed in it, and answer 401.
//
// It is the `X-Version` header now, like every other write's — see
// [`super::common::X_VERSION`]. Nothing about the guarantee changed; it stopped being
// a body field's business to carry it.

/// What `POST /api/user/login` answers a [`crate::requests::user::LoginRequest`] with.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginResponse {
    pub access_token: String,
}

/// What `POST /api/user/session/refresh` answers with. No request body — the refresh
/// token arrives as a cookie.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshResponse {
    pub access_token: String,
}
