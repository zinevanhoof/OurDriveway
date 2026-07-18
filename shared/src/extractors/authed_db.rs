use axum::{
    extract::{FromRef, FromRequestParts},
    http::request::Parts,
};
use axum_extra::{
    TypedHeader,
    headers::{Authorization, authorization::Bearer},
};
use surrealdb::{Surreal, engine::remote::ws::Client};

use crate::error::myerror::{ContextExt, MyError};

pub struct AuthedDb(pub Surreal<Client>);

impl<S> FromRequestParts<S> for AuthedDb
where
    Surreal<Client>: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = MyError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let TypedHeader(Authorization(auth)) =
            TypedHeader::<Authorization<Bearer>>::from_request_parts(parts, state)
                .await
                .context_bad_request(("Bad Request", "Invalid JWT"))?;

        let db = Surreal::<Client>::from_ref(state);
        db.authenticate(auth.token())
            .await
            .context_unauthorized(("Unauthorized", "JWT"))?;

        Ok(AuthedDb(db))
    }
}
