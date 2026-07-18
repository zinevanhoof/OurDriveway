use geo::Point;
use surrealdb::types::{Datetime, Geometry, RecordId, SurrealValue};

use crate::{
    general_models::spot::{Address, Availability},
    requests::spot::CreateSpotRequest,
};

#[derive(SurrealValue)]
pub struct Spot {
    pub id: RecordId,
    pub owner_id: String,
    pub title: String,
    pub description: Option<String>,
    pub price_per_hour: f64,
    pub images: Vec<String>,
    pub location: Point<f64>,

    pub address: Address,
    pub availability: Availability,
    /// IANA name (e.g. "Europe/Brussels") of the spot's location. Availability
    /// times are stored as local wall-clock in this zone, not UTC.
    pub timezone: String,

    pub created_at: Datetime,
    pub updated_at: Datetime,
}

#[derive(SurrealValue)]
pub struct CreateSpot {
    pub title: String,
    pub description: Option<String>,
    pub price_per_hour: f64,
    pub images: Vec<String>,
    pub location: Geometry,

    pub address: Address,
    pub availability: Availability,
    pub timezone: String,
}

impl From<(CreateSpotRequest, Vec<String>, Geometry, String)> for CreateSpot {
    fn from(value: (CreateSpotRequest, Vec<String>, Geometry, String)) -> Self {
        Self {
            title: value.0.title,
            description: value.0.description,
            price_per_hour: value.0.price_per_hour,
            images: value.1,
            location: value.2,
            address: value.0.address.into(),
            availability: value.0.availability.into(),
            timezone: value.3,
        }
    }
}
