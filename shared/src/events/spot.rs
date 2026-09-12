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
    /// Withdrawn for good. Unlike switching the listing off — which is a
    /// `SpotUpdated` carrying nothing but `active` — this also cancels every booking
    /// the spot still owes, because the host is saying they cannot provide the space.
    ///
    /// A soft delete: the row stays so a renter's past bookings keep resolving their
    /// spot's title and address. `deleted` is what hides it everywhere else.
    Deleted {
        spot_id: Uuid,
    },
}

impl SpotEvent {
    /// The spot every variant is about — the aggregate half of `spot:<uuid>`.
    pub fn spot_id(&self) -> Uuid {
        match self {
            Self::Created(e) => e.spot_id,
            Self::Updated(e) => e.spot_id,
            Self::Deleted { spot_id } => *spot_id,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SpotCreated {
    pub spot_id: Uuid,
    /// From the verified JWT claim.
    pub host_id: Uuid,
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
///
/// The live switch is one of these, carrying `active` and nothing else. It used to
/// be its own pair of events; folding it in is what lets the toggle avoid
/// resubmitting `availability`, which is the field booking-service cancels
/// bookings over.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SpotUpdated {
    pub spot_id: Uuid,
    pub title: Option<String>,
    pub description: Option<String>,
    pub price_per_hour_cents: Option<i64>,
    pub images: Option<Vec<String>>,
    pub availability: Option<Availability>,
    /// Off blocks new reservations and hides the spot from search; bookings already
    /// made stay valid and are honoured. That is the whole difference from
    /// [`SpotEvent::Deleted`].
    pub active: Option<bool>,
}

#[cfg(test)]
impl SpotUpdated {
    /// Nothing but the live switch — exactly what the manage screen's toggle sends.
    pub fn live(active: bool) -> Self {
        Self {
            spot_id: Uuid::now_v7(),
            title: None,
            description: None,
            price_per_hour_cents: None,
            images: None,
            availability: None,
            active: Some(active),
        }
    }
}
