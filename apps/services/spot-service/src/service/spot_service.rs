use std::sync::OnceLock;

use geo::Point;
use shared::error::myerror::{ContextExt, MyResult};
use shared::{domain_models::spot::CreateSpot, requests::spot::CreateSpotRequest};
use surrealdb::types::Geometry;
use tzf_rs::DefaultFinder;

use crate::repository::spot_repository::SpotRepository;
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

pub struct SpotService {
    pub spot_repository: SpotRepository,
}

impl SpotService {
    pub async fn create_spot(
        &self,
        request: CreateSpotRequest,
        images: Vec<String>,
    ) -> MyResult<()> {
        // Independently geocode the submitted address (never trust client coords).
        // No confident match -> reject; the frontend renders `detail` from 422s.
        let (lng, lat) = locationiq::geocode(&request.address.formatted)
            .await?
            .context_unprocessable_entity((
                "Address could not be verified",
                "We couldn't locate that address. Please check the fields.",
            ))?;
        let point = Point::new(lng, lat);
        let timezone = timezone_for(point.x(), point.y());
        let location = Geometry::Point(point);
        self.spot_repository
            .create_spot(CreateSpot::from((request, images, location, timezone)))
            .await?;

        Ok(())
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
