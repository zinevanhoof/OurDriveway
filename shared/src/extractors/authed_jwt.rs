use axum::{extract::FromRequestParts, http::request::Parts};
use axum_extra::{
    TypedHeader,
    headers::{Authorization, authorization::Bearer},
};
use jsonwebtoken::{TokenData, Validation, decode};
use uuid::Uuid;

use crate::{
    claims::jwt_claims::{ISSUER, JwtClaims},
    error::myerror::{ContextExt, MyError},
    jwt_decoding_key,
};

/// The only authentication extractor. Verifies the token's signature in Rust and
/// hands back its claims; it never touches the database.
///
/// This replaced `AuthedDb`, which authenticated the *shared* `Surreal<Client>`
/// connection with the caller's JWT — one socket whose session every request
/// mutated in turn. Services now connect once with their own database-level user
/// and authorize in Rust, so nothing about a request can change the connection.
pub struct AuthedJwt {
    /// The caller's uuid, parsed once here from the claim's `user:u'…'` record-id
    /// form. Every handler and service below this point takes a `Uuid` and never
    /// sees that spelling.
    pub user_id: Uuid,
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

        let token_data: TokenData<JwtClaims> =
            decode(auth.token(), jwt_decoding_key(), &validation)
                // The detail string must keep containing "JWT": the frontend's apiFetch
                // (api/king.ts) only attempts a token refresh on a 401 whose `detail`
                // mentions it. Change this wording and silent logouts start happening.
                .context_unauthorized(("Unauthorized", "JWT"))?;

        let claims = token_data.claims;
        // `sub` is already the uuid. It used to be a SurrealDB record id spelled
        // `user:u'<uuid>'` — the form that made `$auth` resolve for the GraphQL
        // permission clauses — which had to be parsed apart here, with a
        // `parse_claim_id` and a unit test covering the shapes an attacker might send.
        // Both are gone with the claim.
        let user_id = claims.sub;

        Ok(AuthedJwt { user_id, claims })
    }
}
