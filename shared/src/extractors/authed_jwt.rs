use axum::{extract::FromRequestParts, http::request::Parts};
use axum_extra::{
    TypedHeader,
    headers::{Authorization, authorization::Bearer},
};
use jsonwebtoken::{DecodingKey, TokenData, Validation, decode};

use crate::{
    SHARED_CONFIG,
    claims::jwt_claims::JwtClaims,
    error::myerror::{ContextExt, MyError},
};

pub struct AuthedJwt {
    pub user_id: String,
}

impl<S> FromRequestParts<S> for AuthedJwt
where
    S: Send + Sync,
{
    type Rejection = MyError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let TypedHeader(Authorization(auth)) =
            TypedHeader::<Authorization<Bearer>>::from_request_parts(parts, state)
                .await
                .context_bad_request(("Bad Request", "Invalid JWT"))?;

        let token_data: TokenData<JwtClaims> = decode(
            auth.token(),
            &DecodingKey::from_secret(SHARED_CONFIG.jwt_secret.as_bytes()),
            &Validation::default(),
        )
        .context_unauthorized(("Unauthorized", "JWT"))?;

        Ok(AuthedJwt {
            user_id: token_data.claims.id,
        })
    }
}
