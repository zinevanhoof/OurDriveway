use axum::{extract::FromRequestParts, http::request::Parts};
use axum_extra::{
    TypedHeader,
    headers::{Authorization, authorization::Bearer},
};
use jsonwebtoken::{DecodingKey, TokenData, Validation, decode};

use crate::{
    SHARED_CONFIG,
    claims::jwt_claims::{ISSUER, JwtClaims},
    error::myerror::{ContextExt, MyError},
};

/// The only authentication extractor. Verifies the token's signature in Rust and
/// hands back its claims; it never touches the database.
///
/// This replaced `AuthedDb`, which authenticated the *shared* `Surreal<Client>`
/// connection with the caller's JWT — one socket whose session every request
/// mutated in turn. Services now connect once with their own database-level user
/// and authorize in Rust, so nothing about a request can change the connection.
pub struct AuthedJwt {
    /// SurrealDB record id in SQL form (`"user:abc123"`), i.e. the `$token.ID`
    /// that row-level permissions compare against. Same string, so values stored
    /// under either scheme stay comparable.
    pub user_id: String,
    pub claims: JwtClaims,
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

        let mut validation = Validation::default();
        // Reject tokens minted by anything but us, and honour `nbf`. Both claims
        // are already set by generate_jwt; they were simply never checked.
        // Validation::leeway (60s default) absorbs clock skew between services.
        validation.set_issuer(&[ISSUER]);
        validation.validate_nbf = true;

        let token_data: TokenData<JwtClaims> = decode(
            auth.token(),
            &DecodingKey::from_secret(SHARED_CONFIG.jwt_secret.as_bytes()),
            &validation,
        )
        // The detail string must keep containing "JWT": the frontend's apiFetch
        // (api/king.ts) only attempts a token refresh on a 401 whose `detail`
        // mentions it. Change this wording and silent logouts start happening.
        .context_unauthorized(("Unauthorized", "JWT"))?;

        let claims = token_data.claims;
        Ok(AuthedJwt {
            user_id: claims.id.clone(),
            claims,
        })
    }
}
