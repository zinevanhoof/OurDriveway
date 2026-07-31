use std::sync::OnceLock;

use async_nats::jetstream::Context;
use geo::Point;
use shared::error::myerror::{ContextExt, MyResult};
use shared::events::spot::{SpotCreated, SpotEvent};
use shared::events::{Envelope, shard_of, spot_subject};
use shared::requests::spot::CreateSpotRequest;
use tzf_rs::DefaultFinder;
use uuid::Uuid;

use crate::service::locationiq;

/// IANA timezone name for a point. `lng`/`lat` order (geo `Point`: x = lng,
/// y = lat). Falls back to "Etc/UTC" when the point matches no zone (e.g. open
/// ocean, or the hardcoded placeholder coords until geocoding is wired).
fn timezone_for(lng: f64, lat: f64) -> String {
    static FINDER: OnceLock<DefaultFinder> = OnceLock::new();
    let name = FINDER.get_or_init(DefaultFinder::new).get_tz_name(lng, lat);
    if name.is_empty() {
        "Etc/UTC".to_string()
    } else {
        name.to_string()
    }
}

/// Write side. Validates, then publishes — it never touches the database.
///
/// The event is the commit: this instance's projector applies it a moment later,
/// as does every other instance's. Writing locally *and* publishing would be a
/// dual write with no atomicity, and the copies drift the first time one fails.
pub struct SpotService {
    pub js: Context,
}

pub struct Created {
    pub spot_id: Uuid,
    /// Stream sequence the event landed at — returned to the client so a
    /// follow-up read can wait for its own write to be projected.
    pub seq: u64,
}

impl SpotService {
    pub async fn create_spot(
        &self,
        request: CreateSpotRequest,
        owner_id: String,
        images: Vec<String>,
    ) -> MyResult<Created> {
        // Independently geocode the submitted address (never trust client coords).
        // No confident match -> reject; the frontend renders `detail` from 422s.
        let (lng, lat) = locationiq::geocode(&request.address.formatted)
            .await?
            .context_unprocessable_entity((
                "Address could not be verified",
                "We couldn't locate that address. Please check the fields.",
            ))?;
        let point = Point::new(lng, lat);

        // Id and shard are minted here, before publishing. A database-generated id
        // would differ on every replica applying this same event.
        let spot_id = Uuid::now_v7();
        let shard = shard_of(&spot_id);

        let event = SpotEvent::Created(SpotCreated {
            spot_id,
            shard: shard.clone(),
            owner_id: owner_id.clone(),
            title: request.title,
            description: request.description,
            price_per_hour_cents: request.price_per_hour_cents,
            images,
            lng: point.x(),
            lat: point.y(),
            address: request.address.into(),
            availability: request.availability.into(),
            timezone: timezone_for(point.x(), point.y()),
        });

        let seq = bus::publish(
            &self.js,
            spot_subject(&shard, &spot_id),
            &Envelope::new(event, Some(owner_id)),
        )
        .await?;

        Ok(Created { spot_id, seq })
    }
}

#[cfg(test)]
mod tests {
    use super::timezone_for;

    #[test]
    fn derives_zone_from_coords() {
        // Brussels (lng, lat).
        assert_eq!(timezone_for(4.35, 50.85), "Europe/Brussels");
        // Open ocean resolves to a nautical Etc/GMT zone (tzf-rs never returns
        // empty); the is_empty -> "Etc/UTC" guard stays as a defensive fallback.
        assert_eq!(timezone_for(-40.0, 0.0), "Etc/GMT+3");
    }
}
