use std::sync::{Arc, OnceLock};

use async_nats::jetstream::Context;
use axum::http::StatusCode;
use geo::Point;
use shared::error::myerror::{ContextExt, MyError, MyResult};
use shared::events::spot::{SpotCreated, SpotEvent, SpotUpdated};
use shared::events::{Envelope, shard_of, spot_subject};
use shared::requests::spot::{CreateSpotRequest, UpdateSpotRequest};
use tzf_rs::DefaultFinder;
use uuid::Uuid;

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

/// Write side. Validates, then publishes — it never touches the database.
///
/// The event is the commit: this instance's projector applies it a moment later,
/// as does every other instance's. Writing locally *and* publishing would be a
/// dual write with no atomicity, and the copies drift the first time one fails.
pub struct SpotService {
    pub js: Context,
    /// Read-only here. Edits need the spot's owner to authorize and its shard to
    /// address the subject — both of which only the projection knows.
    pub repository: Arc<SpotRepository>,
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
        owner_id: Uuid,
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
            owner_id,
            title: request.title,
            description: request.description,
            price_per_hour_cents: request.price_per_hour_cents,
            images: request.images,
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

    /// An edit of an existing listing. Every field the host can change is carried,
    /// so `SpotUpdated`'s "None means unchanged" is unused here — the form always
    /// submits its whole state, and letting the client omit fields would make it
    /// the one deciding what counts as a change.
    pub async fn update_spot(
        &self,
        spot_id: &Uuid,
        request: UpdateSpotRequest,
        owner_id: Uuid,
    ) -> MyResult<u64> {
        let (id, shard) = self.owned(spot_id, &owner_id).await?;

        let event = SpotEvent::Updated(SpotUpdated {
            spot_id: id,
            title: Some(request.title),
            description: request.description,
            price_per_hour_cents: Some(request.price_per_hour_cents),
            images: Some(request.images),
            availability: Some(request.availability.into()),
        });

        self.publish(&id, &shard, event, owner_id).await
    }

    /// The live switch. Off blocks new reservations; bookings already taken are
    /// honoured, which is the whole difference from a delete.
    pub async fn set_active(&self, spot_id: &Uuid, active: bool, owner_id: Uuid) -> MyResult<u64> {
        let (id, shard) = self.owned(spot_id, &owner_id).await?;
        let event = if active {
            SpotEvent::Activated { spot_id: id }
        } else {
            SpotEvent::Deactivated { spot_id: id }
        };
        self.publish(&id, &shard, event, owner_id).await
    }

    /// Withdraw the listing for good. booking-service reacts to this by cancelling
    /// every booking the spot still owes — nothing here needs to know that, or to
    /// know bookings exist at all.
    pub async fn delete_spot(&self, spot_id: &Uuid, owner_id: Uuid) -> MyResult<u64> {
        let (id, shard) = self.owned(spot_id, &owner_id).await?;
        self.publish(&id, &shard, SpotEvent::Deleted { spot_id: id }, owner_id)
            .await
    }

    /// Resolves a spot the caller is allowed to write to, returning its id and the
    /// shard its events live on.
    ///
    /// Not-yours is a 404, not a 403: whether a spot id exists isn't this caller's
    /// business. Same choice as booking-service's `authorize`.
    async fn owned(&self, spot_id: &Uuid, owner_id: &Uuid) -> MyResult<(Uuid, String)> {
        let not_found = || {
            MyError::api(
                StatusCode::NOT_FOUND,
                "Not Found",
                "That spot doesn't exist.",
            )
        };

        let spot = self
            .repository
            .spot_for_update(spot_id)
            .await?
            .ok_or_else(not_found)?;

        // A deleted spot is gone as far as the host is concerned, even though the
        // row survives for the bookings that reference it.
        if spot.owner_id != *owner_id || spot.deleted {
            return Err(not_found());
        }

        Ok((*spot_id, spot.shard))
    }

    /// No compare-and-swap on any of these.
    ///
    /// The tempting reason to want one is a host narrowing availability while a
    /// booking lands — but CAS is per-subject, and bookings live on the BOOKINGS
    /// stream, so no assertion made here can see them. That ordering is enforced
    /// where it exists: booking-service's per-spot total order. What's left is two
    /// concurrent edits by the one owner, where last-write-wins is the honest answer.
    async fn publish(
        &self,
        spot_id: &Uuid,
        shard: &str,
        event: SpotEvent,
        owner_id: Uuid,
    ) -> MyResult<u64> {
        bus::publish(
            &self.js,
            spot_subject(shard, spot_id),
            &Envelope::new(event, Some(owner_id)),
        )
        .await
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
