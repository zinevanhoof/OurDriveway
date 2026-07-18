use chrono::{DateTime, Utc};
use jsonwebtoken::{EncodingKey, Header, encode};
use shared::{claims::jwt_claims::JwtClaims, error::myerror::MyResult};
use surrealdb::types::{RecordId, ToSql};
use uuid::Uuid;

use crate::CONFIG;

pub fn generate_jwt(user_id: &RecordId, exp: DateTime<Utc>, jti: Uuid) -> MyResult<String> {
    let now = Utc::now();
    let now_timestamp = now.timestamp();
    let exp_timestamp = exp.timestamp();

    let claims = JwtClaims {
        iat: now_timestamp,
        nbf: now_timestamp,
        exp: exp_timestamp,
        iss: "OurDriveway".to_string(),
        jti: jti,

        ns: "main".to_string(),
        db: "main".to_string(),
        ac: "account".to_string(),

        id: user_id.to_sql(),
    };

    let jwt = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(CONFIG.jwt_secret.as_bytes()),
    )?;

    Ok(jwt)
}
