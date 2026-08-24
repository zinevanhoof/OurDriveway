use chrono::{DateTime, Duration, Utc};
use jsonwebtoken::{EncodingKey, Header, encode};
use shared::{
    claims::jwt_claims::{ISSUER, JwtClaims},
    error::myerror::MyResult,
};
use uuid::Uuid;

use crate::CONFIG;

/// Mints an access token for a user, and returns the two things its session needs
/// alongside it: when the *refresh* token that accompanies it should expire, and
/// the `jti` binding the pair together.
///
/// Both lifetimes come from `CONFIG` and are read here rather than by the caller,
/// so there is one place that decides how long a session lives.
pub fn mint(user_id: &Uuid) -> MyResult<(String, DateTime<Utc>, Uuid)> {
    let now = Utc::now();
    let exp = now + Duration::minutes(CONFIG.jwt_expiration);
    let jti = Uuid::new_v4();

    let claims = JwtClaims {
        iat: now.timestamp(),
        nbf: now.timestamp(),
        exp: exp.timestamp(),
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

    Ok((
        jwt,
        now + Duration::days(CONFIG.refresh_token_expiration),
        jti,
    ))
}
