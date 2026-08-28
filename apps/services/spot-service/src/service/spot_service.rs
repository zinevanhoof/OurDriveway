use chrono::Utc;
use shared::domain_models::spot::{Spot, SpotPatch};
use shared::error::myerror::{ContextExt, MyResult};
use shared::events::spot::{SpotCreated, SpotEvent, SpotUpdated};
use bus::outbox;
use shared::db;
use shared::events::{Envelope, aggregate_id, format_version, spot_subject};
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

/// Write side. Validates, writes its own rows, and enqueues the event beside them
/// — all in one transaction.
///
/// The database is authoritative: this commit *is* the write, and the event in
/// `_outbox` is a durable side effect of the same commit that the relay carries to
/// NATS afterwards. It used to be the other way round — publish, and let this
/// service's own projector apply the row a moment later.
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
    /// Read-only here. Edits need the spot's owner to authorize against, which
    /// only the projection knows.
    pub spots: SpotRepository,
}

impl SpotService {
    /// Returns only the await token (`spot:<id>@<version>`), like every other
    /// write here.
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
    pub async fn create_spot(&self, owner_id: &Uuid, request: CreateSpotRequest) -> MyResult<String> {
        // Independently geocode the submitted address (never trust client coords).
        // No confident match -> reject; the frontend renders `detail` from 422s.
        let (lng, lat) = locationiq::geocode(&request.address.formatted)
            .await?
            .context_unprocessable_entity((
                "Address could not be verified",
                "We couldn't locate that address. Please check the fields.",
            ))?;

        // Minted here, before publishing. A database-generated id would differ on
        // every replica applying this same event.
        let spot_id = Uuid::now_v7();

        let created = SpotCreated {
            spot_id,
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
        };

        // Row and event in one transaction — the database is authoritative now, and
        // the event is a durable side effect of the same commit.
        let now = Utc::now();
        let tx = db::begin(&self.spots.q).await?;
        let version = db::next_version(&tx, "spot", &spot_id).await?;

        // No `set_version` after this: the row carries its own version and this is
        // a whole-row write. `update_spot` and `delete_spot` still need it — they
        // patch, and a patch does not touch the column.
        SpotRepository { q: &tx }
            .upsert(Spot::created(created.clone(), now, version))
            .await?;

        let envelope = Envelope::new(
            SpotEvent::Created(created),
            Some(*owner_id),
            aggregate_id("spot", &spot_id),
            version,
        );
        let await_token = format_version(&envelope.aggregate, envelope.version);

        outbox::enqueue(&tx, &spot_subject(&spot_id), &envelope).await?;
        tx.commit().await?;

        Ok(await_token)
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
    ) -> MyResult<String> {
        let spot = self.owned(owner_id, spot_id).await?;

        let updated = SpotUpdated {
            spot_id: spot.id,
            title: request.title.map(Into::into),
            description: request.description.map(Into::into),
            price_per_hour_cents: request.price_per_hour_cents,
            images: request.images,
            availability: request.availability,
            active: request.active,
        };

        let now = Utc::now();
        let tx = db::begin(&self.spots.q).await?;
        let version = db::next_version(&tx, "spot", &spot.id).await?;

        SpotRepository { q: &tx }
            .patch(spot.id, SpotPatch::updated(updated.clone(), now))
            .await?;
        db::set_version(&tx, "spot", &spot.id, version).await?;

        let envelope = Envelope::new(
            SpotEvent::Updated(updated),
            Some(*owner_id),
            aggregate_id("spot", &spot.id),
            version,
        );
        let await_token = format_version(&envelope.aggregate, envelope.version);

        outbox::enqueue(&tx, &spot_subject(&spot.id), &envelope).await?;
        tx.commit().await?;

        Ok(await_token)
    }

    /// Withdraw the listing for good. booking-service reacts to this by cancelling
    /// every booking the spot still owes — nothing here needs to know that, or to
    /// know bookings exist at all.
    pub async fn delete_spot(&self, owner_id: &Uuid, spot_id: &Uuid) -> MyResult<String> {
        let spot = self.owned(owner_id, spot_id).await?;
        let now = Utc::now();
        let tx = db::begin(&self.spots.q).await?;
        let version = db::next_version(&tx, "spot", &spot.id).await?;

        // A soft delete: the row survives so a renter's past bookings still resolve
        // a title and an address. Every list filters `deleted`.
        SpotRepository { q: &tx }
            .patch(spot.id, SpotPatch::deleted(now))
            .await?;
        db::set_version(&tx, "spot", &spot.id, version).await?;

        let envelope = Envelope::new(
            SpotEvent::Deleted { spot_id: spot.id },
            Some(*owner_id),
            aggregate_id("spot", &spot.id),
            version,
        );
        let await_token = format_version(&envelope.aggregate, envelope.version);

        outbox::enqueue(&tx, &spot_subject(&spot.id), &envelope).await?;
        tx.commit().await?;

        Ok(await_token)
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

    /// Re-emits every spot as the events that reproduce its current row, for a
    /// consumer that needs rebuilding. See [`outbox::backfill`] for what this is and
    /// is not.
    ///
    /// One to three events each. `Created` carries the whole listing, so the extras
    /// exist only for the two states it cannot express: a listing switched off, and
    /// one withdrawn.
    ///
    /// Note what the `Updated` deliberately leaves out — `availability`. That is the
    /// field booking-service cancels bookings over, and an edit carrying it makes
    /// this projector re-examine every confirmed booking on the spot. Carrying only
    /// `active` is the same shape the manage screen's toggle sends, and it is a
    /// no-op there for exactly the same reason.
    pub async fn backfill(&self) -> MyResult<usize> {
        let mut sent = 0;

        for spot in self.spots.all().await? {
            let spot_id = spot.id;
            let (created_at, updated_at) = (spot.created_at.into(), spot.updated_at.into());
            let (active, deleted) = (spot.active, spot.deleted);

            let created = SpotCreated {
                spot_id,
                owner_id: spot.owner_id,
                title: spot.title,
                description: spot.description,
                price_per_hour_cents: spot.price_per_hour,
                images: spot.images,
                // The stored point, back to the pair the event carries. x is lng,
                // y is lat — geo's order, and the one this was built from.
                lng: spot.location.x(),
                lat: spot.location.y(),
                address: spot.address,
                availability: spot.availability,
                timezone: spot.timezone,
            };

            let mut events = vec![(created_at, SpotEvent::Created(created))];

            // `Created` always lands active, so a listing that is off needs the edit
            // that switched it off replayed after it.
            if !active {
                events.push((
                    updated_at,
                    SpotEvent::Updated(SpotUpdated {
                        spot_id,
                        title: None,
                        description: None,
                        price_per_hour_cents: None,
                        images: None,
                        availability: None,
                        active: Some(false),
                    }),
                ));
            }

            // Last, because it is the terminal state and the projections apply these
            // in order. booking-service reacts to this by cancelling what the spot
            // still owes — already done the first time round, and its transition is
            // scoped to `confirmed`, so a second pass matches nothing.
            if deleted {
                events.push((updated_at, SpotEvent::Deleted { spot_id }));
            }

            sent += outbox::backfill(
                &self.spots.q,
                &spot_subject(&spot_id),
                &aggregate_id("spot", &spot_id),
                spot.version,
                events,
            )
            .await?;
        }

        tracing::info!(events = sent, "spots backfilled");
        Ok(sent)
    }
}
