use serde::Serialize;

/// What login and refresh answer with.
///
/// `seq` is the SESSIONS position the session event landed at, in the same
/// `STREAM:seq` form every 202 uses — see `bus::format_seq`. It is here because
/// the other half of a session, the refresh token, goes into an httponly cookie
/// the client cannot read: without this field there would be nothing for the
/// client to echo, and `/refresh` could legitimately read the projection before
/// the session it is rotating has landed in it, answering 401.
#[derive(Serialize)]
pub struct AuthResponse {
    access_token: String,
    seq: String,
}

impl AuthResponse {
    pub fn new(access_token: String, seq: String) -> Self {
        Self { access_token, seq }
    }
}
