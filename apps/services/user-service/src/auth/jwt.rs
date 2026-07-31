use chrono::{DateTime, Utc};
use jsonwebtoken::{EncodingKey, Header, encode};
use shared::{
    claims::jwt_claims::{ISSUER, JwtClaims},
    error::myerror::MyResult,
};
use uuid::Uuid;

use crate::CONFIG;

/// `claim_id` is the `user:<uuid>` string from `user_claim_id` — built once,
/// in one place, so it always matches the strings stored in other services.
pub fn generate_jwt(claim_id: &str, exp: DateTime<Utc>, jti: Uuid) -> MyResult<String> {
    let now = Utc::now();
    let now_timestamp = now.timestamp();
    let exp_timestamp = exp.timestamp();

    let claims = JwtClaims {
        iat: now_timestamp,
        nbf: now_timestamp,
        exp: exp_timestamp,
        iss: ISSUER.to_string(),
        jti,

        ns: "main".to_string(),
        db: "main".to_string(),
        ac: "account".to_string(),

        id: claim_id.to_string(),
    };

    let jwt = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(CONFIG.jwt_secret.as_bytes()),
    )?;

    Ok(jwt)
}
