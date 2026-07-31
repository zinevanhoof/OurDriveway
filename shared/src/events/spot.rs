use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::general_models::spot::{Address, Availability};

/// Everything that can happen to a spot. Published by spot-service only.
///
/// Each variant carries every field a projector needs to rebuild the row without
/// consulting anything else — projections must be pure functions of the log.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SpotEvent {
    Created(SpotCreated),
    Updated(SpotUpdated),
    Deactivated { spot_id: Uuid },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SpotCreated {
    pub spot_id: Uuid,
    /// Assigned once, here, and echoed by every later event for this spot so its
    /// subject never moves. Recomputing it downstream would break if SHARD_COUNT
    /// ever changed.
    pub shard: String,
    /// `"user:abc"`, from the verified JWT claim.
    pub owner_id: String,
    pub title: String,
    pub description: Option<String>,
    /// EUR cents. Never a float: this feeds the amount a renter is charged, and
    /// binary floating point cannot represent most decimal prices exactly.
    pub price_per_hour_cents: i64,
    pub images: Vec<String>,
    /// Plain floats rather than a `Geometry`: the projector rebuilds the point
    /// with `type::point([$lng, $lat])`, so the event never depends on a database
    /// type's wire format.
    pub lng: f64,
    pub lat: f64,
    pub address: Address,
    pub availability: Availability,
    /// IANA name, derived from the geocoded point at creation time.
    pub timezone: String,
}

/// Partial update — `None` means "leave alone", not "clear".
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SpotUpdated {
    pub spot_id: Uuid,
    pub title: Option<String>,
    pub description: Option<String>,
    pub price_per_hour_cents: Option<i64>,
    pub images: Option<Vec<String>>,
    pub availability: Option<Availability>,
}
