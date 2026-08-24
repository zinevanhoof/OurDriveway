use async_nats::jetstream::Context;
use shared::domain_models::spot::Spot;
use shared::error::myerror::{ContextExt, MyResult};
use shared::events::spot::{SpotCreated, SpotEvent, SpotUpdated};
use shared::events::{Envelope, shard_of, spot_subject};
use shared::requests::spot::{CreateSpotRequest, UpdateSpotRequest};
use uuid::Uuid;

use crate::client::locationiq;
use crate::policy::timezone;
use crate::repository::spot_repository::SpotRepository;

/// Answered for a spot that does not exist, one the caller does not own, and one
/// already deleted — all three, deliberately, and all three a 404.
///
/// Whether a spot id exists is not this caller's business, so a 403 would leak
/// exactly what the 404 withholds. Same choice as booking-service's `authorize`.
const NOT_FOUND: (&str, &str) = ("Not Found", "That spot doesn't exist.");

/// Write side. Validates, then publishes — it never writes to the database.
///
/// The event is the commit: this instance's projector applies it a moment later,
/// as does every other instance's. Writing locally *and* publishing would be a
/// dual write with no atomicity, and the copies drift the first time one fails.
///
/// Every method takes the caller's id first, then what it is acting on, then the
/// request body. The id comes from the verified JWT and never from the body —
/// that is the whole reason it is a separate argument.
///
/// No compare-and-swap on any write here. The tempting reason to want one is a host
/// narrowing availability while a booking lands — but CAS is per-subject, and bookings
/// live on the BOOKINGS stream, so no assertion made here can see them. That ordering is
/// enforced where it exists: booking-service's per-spot total order. What's left is two
/// concurrent edits by the one owner, where last-write-wins is the honest answer.
pub struct SpotService {
    pub js: Context,
    /// Read-only here. Edits need the spot's owner to authorize and its shard to
    /// address the subject — both of which only the projection knows.
    pub spots: SpotRepository,
}

impl SpotService {
    /// Returns only the stream sequence, like every other write here.
    ///
    /// The minted `spot_id` is deliberately not returned. Nothing asks for it: the
    /// create form navigates to the list and refetches, and the id would be a
    /// second thing the response means. booking-service's `reserve` does return
    /// one, because checkout is started from it before any projection could have
    /// caught up — that reason does not exist here.
    ///
    /// It is unrecoverable if wanted later, though: the id is minted below rather
    /// than by the client, so "open the spot I just made" would need this to hand
    /// it back again.
    pub async fn create_spot(&self, owner_id: &Uuid, request: CreateSpotRequest) -> MyResult<u64> {
        // Independently geocode the submitted address (never trust client coords).
        // No confident match -> reject; the frontend renders `detail` from 422s.
        let (lng, lat) = locationiq::geocode(&request.address.formatted)
            .await?
            .context_unprocessable_entity((
                "Address could not be verified",
                "We couldn't locate that address. Please check the fields.",
            ))?;

        // Id and shard are minted here, before publishing. A database-generated id
        // would differ on every replica applying this same event.
        let spot_id = Uuid::now_v7();
        let shard = shard_of(&spot_id);

        let event = SpotEvent::Created(SpotCreated {
            spot_id,
            shard: shard.clone(),
            owner_id: *owner_id,
            title: request.title.into_inner(),
            description: request.description.map(Into::into),
            price_per_hour_cents: request.price_per_hour_cents,
            images: request.images,
            lng,
            lat,
            address: request.address,
            availability: request.availability,
            timezone: timezone::for_coords(lng, lat),
        });

        bus::publish(
            &self.js,
            spot_subject(&shard, &spot_id),
            &Envelope::new(event, Some(*owner_id)),
        )
        .await
    }

    /// An edit of an existing listing, and also the live switch — that is one
    /// request carrying nothing but `active`, which is why every field here is
    /// optional and `None` reaches the projections as "leave alone".
    ///
    /// The edit form still submits its whole state, so nothing about a save changed.
    pub async fn update_spot(
        &self,
        owner_id: &Uuid,
        spot_id: &Uuid,
        request: UpdateSpotRequest,
    ) -> MyResult<u64> {
        let spot = self.owned(owner_id, spot_id).await?;

        let event = SpotEvent::Updated(SpotUpdated {
            spot_id: spot.id,
            title: request.title.map(Into::into),
            description: request.description.map(Into::into),
            price_per_hour_cents: request.price_per_hour_cents,
            images: request.images,
            availability: request.availability,
            active: request.active,
        });

        bus::publish(
            &self.js,
            spot_subject(&spot.shard, &spot.id),
            &Envelope::new(event, Some(*owner_id)),
        )
        .await
    }

    /// Withdraw the listing for good. booking-service reacts to this by cancelling
    /// every booking the spot still owes — nothing here needs to know that, or to
    /// know bookings exist at all.
    pub async fn delete_spot(&self, owner_id: &Uuid, spot_id: &Uuid) -> MyResult<u64> {
        let spot = self.owned(owner_id, spot_id).await?;
        let event = SpotEvent::Deleted { spot_id: spot.id };
        bus::publish(
            &self.js,
            spot_subject(&spot.shard, &spot.id),
            &Envelope::new(event, Some(*owner_id)),
        )
        .await
    }

    /// Resolves a spot the caller is allowed to write to.
    ///
    /// Returns the whole row rather than the two columns the callers use: it is one
    /// point read either way, and the alternative was a second projection shape to
    /// keep in step with this table.
    ///
    /// A `None` here means this instance has not projected the spot yet, which the
    /// caller must treat as "not found" rather than "not yours" — and does, since
    /// both answer [`NOT_FOUND`].
    async fn owned(&self, owner_id: &Uuid, spot_id: &Uuid) -> MyResult<Spot> {
        let spot = self
            .spots
            .find_by_id(*spot_id)
            .await?
            .context_not_found(NOT_FOUND)?;

        // A deleted spot is gone as far as the host is concerned, even though the
        // row survives for the bookings that reference it.
        (spot.owner_id == *owner_id && !spot.deleted).context_not_found(NOT_FOUND)?;

        Ok(spot)
    }
}
