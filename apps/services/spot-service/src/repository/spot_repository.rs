use shared::domain_models::spot::{CreateSpot, Spot};
use shared::error::myerror::{ContextExt, MyResult};
use surrealdb::{Surreal, engine::remote::ws::Client};

pub struct SpotRepository {
    pub db: Surreal<Client>,
}

impl SpotRepository {
    pub async fn create_spot(&self, create_spot: CreateSpot) -> MyResult<Spot> {
        let spot: Spot = self
            .db
            .create("spot")
            .content(create_spot)
            .await?
            .take()
            .context_internal("spot create returned nothing")?;

        Ok(spot)
    }
}
