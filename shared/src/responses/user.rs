use serde::Serialize;

#[derive(Serialize)]
pub struct AuthResponse {
    access_token: String,
}

impl AuthResponse {
    pub fn new(access_token: String) -> Self {
        Self { access_token }
    }
}
