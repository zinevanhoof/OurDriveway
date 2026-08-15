use chrono::{DateTime, Utc};
use jsonwebtoken::{EncodingKey, Header, encode};
use shared::{
    claims::jwt_claims::{ISSUER, JwtClaims},
    error::myerror::MyResult,
};
use uuid::Uuid;

use crate::CONFIG;

pub fn generate_jwt(user_id: &Uuid, exp: DateTime<Utc>, jti: Uuid) -> MyResult<String> {
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

        // The only `user:` left in the codebase. SurrealDB parses this claim with
        // `syn::record_id` and errors if it fails, so the uuid needs its `u'…'`
        // literal form — that is what makes `$auth` a *uuid-keyed* record id, and
        // what lets every permission clause compare `record::id($auth)` against a
        // `TYPE uuid` field with no cast. A bare uuid here does not parse.
        id: format!("user:u'{user_id}'"),
    };

    let jwt = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(CONFIG.jwt_secret.as_bytes()),
    )?;

    Ok(jwt)
}
