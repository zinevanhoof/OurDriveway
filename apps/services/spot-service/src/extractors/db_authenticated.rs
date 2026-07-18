use axum::{
    extract::{FromRef, FromRequestParts},
    http::request::Parts,
};
use shared::{error::myerror::MyError, extractors::authed_db::AuthedDb};
use surrealdb::{Surreal, engine::remote::ws::Client};

use crate::{repository::spot_repository::SpotRepository, service::spot_service::SpotService};

pub struct DbAuthenticated(pub SpotService);

impl<S> FromRequestParts<S> for DbAuthenticated
where
    Surreal<Client>: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = MyError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let AuthedDb(db) = AuthedDb::from_request_parts(parts, state).await?;
        Ok(DbAuthenticated(SpotService {
            spot_repository: SpotRepository { db },
        }))
    }
}
