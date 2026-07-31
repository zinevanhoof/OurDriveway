use shared::{
    error::myerror::MyResult,
    events::{
        Envelope,
        spot::{SpotCreated, SpotEvent, SpotUpdated},
        user::{UserEvent, UserRegistered, UserUpdated, record_key, user_claim_id},
    },
};
use surrealdb::{Surreal, engine::remote::ws::Client, types::Datetime};
use uuid::Uuid;

/// Writes the combined read model.
///
/// Uses the service's own database-level connection, whose role bypasses the
/// `FOR create, update, delete NONE` on every table — that clause exists to stop
/// the *browser*, which reaches the same database through the GraphQL proxy with
/// only a record identity.
pub struct ViewRepository {
    pub db: Surreal<Client>,
}

impl ViewRepository {
    pub async fn last_seq(&self, stream: &str) -> MyResult<u64> {
        let seq: Option<i64> = self
            .db
            .query("SELECT VALUE last_seq FROM ONLY type::record('_projection', $s)")
            .bind(("s", stream.to_string()))
            .await?
            .take(0)?;
        Ok(seq.unwrap_or(0).max(0) as u64)
    }

    // ─── users ──────────────────────────────────────────────────────────────

    pub async fn apply_user(&self, envelope: Envelope<UserEvent>, seq: u64) -> MyResult<()> {
        let at = envelope.occurred_at;
        match envelope.payload {
            UserEvent::Registered(e) => self.user_registered(e, at, seq).await,
            UserEvent::Updated(e) => self.user_updated(e, at, seq).await,
        }
    }

    async fn user_registered(
        &self,
        e: UserRegistered,
        at: chrono::DateTime<chrono::Utc>,
        seq: u64,
    ) -> MyResult<()> {
        // Note what is absent: no email, no password hash. This table is
        // world-readable, so the projection is the place that decides what can
        // possibly leak — not a permission clause someone might later relax.
        //
        // The two UPDATEs are the backfill. Streams have no cross-stream
        // ordering, so a spot may already be sitting here with `owner = NONE`
        // pointing at a user that hadn't arrived yet. Idempotent, so replaying
        // or racing another consumer is harmless.
        self.db
            .query(
                "BEGIN;
                 UPSERT type::record('user', $id) CONTENT {
                     first_name: $first_name, last_name: $last_name,
                     profile_picture: NONE
                 };
                 UPDATE spot SET owner = type::record('user', $id)
                     WHERE owner_id = $claim AND owner = NONE;
                 UPDATE booking SET renter = type::record('user', $id)
                     WHERE renter_id = $claim AND renter = NONE;
                 UPSERT _projection:USERS SET last_seq = $seq, updated_at = $at;
                 COMMIT;",
            )
            .bind(("id", record_key(&e.user_id)))
            .bind(("claim", user_claim_id(&e.user_id)))
            .bind(("first_name", e.first_name))
            .bind(("last_name", e.last_name))
            .bind(("at", Datetime::from(at)))
            .bind(("seq", seq as i64))
            .await?
            .check()?;
        Ok(())
    }

    async fn user_updated(
        &self,
        e: UserUpdated,
        at: chrono::DateTime<chrono::Utc>,
        seq: u64,
    ) -> MyResult<()> {
        self.db
            .query(
                "BEGIN;
                 UPDATE type::record('user', $id) SET
                     first_name      = $first_name      ?? first_name,
                     last_name       = $last_name       ?? last_name,
                     profile_picture = $profile_picture ?? profile_picture;
                 UPSERT _projection:USERS SET last_seq = $seq, updated_at = $at;
                 COMMIT;",
            )
            .bind(("id", record_key(&e.user_id)))
            .bind(("first_name", e.first_name))
            .bind(("last_name", e.last_name))
            .bind(("profile_picture", e.profile_picture))
            .bind(("at", Datetime::from(at)))
            .bind(("seq", seq as i64))
            .await?
            .check()?;
        Ok(())
    }

    // ─── spots ──────────────────────────────────────────────────────────────

    pub async fn apply_spot(&self, envelope: Envelope<SpotEvent>, seq: u64) -> MyResult<()> {
        let at = envelope.occurred_at;
        match envelope.payload {
            SpotEvent::Created(e) => self.spot_created(e, at, seq).await,
            SpotEvent::Updated(e) => self.spot_updated(e, at, seq).await,
            SpotEvent::Deactivated { spot_id } => self.spot_deactivated(spot_id, at, seq).await,
        }
    }

    async fn spot_created(
        &self,
        e: SpotCreated,
        at: chrono::DateTime<chrono::Utc>,
        seq: u64,
    ) -> MyResult<()> {
        // `owner` resolves to NONE when the owner's UserRegistered hasn't been
        // applied yet; the user projector backfills it on arrival. `owner_id` is
        // always written, and that is what permissions and filters use.
        self.db
            .query(
                "BEGIN;
                 UPSERT type::record('spot', $id) CONTENT {
                     owner: (SELECT VALUE id FROM ONLY user
                             WHERE record::id(id) = $owner_uuid LIMIT 1),
                     owner_id: $owner_id, title: $title, description: $description,
                     price_per_hour: $price, images: $images,
                     location: type::point([$lng, $lat]), active: true,
                     address: $address, availability: $availability,
                     timezone: $timezone, created_at: $at, updated_at: $at
                 };
                 UPSERT _projection:SPOTS SET last_seq = $seq, updated_at = $at;
                 COMMIT;",
            )
            .bind(("id", record_key(&e.spot_id)))
            .bind(("owner_id", e.owner_id.clone()))
            // owner_id is "user:<uuid>"; the record key is just the uuid.
            .bind((
                "owner_uuid",
                e.owner_id.strip_prefix("user:").unwrap_or("").to_string(),
            ))
            .bind(("title", e.title))
            .bind(("description", e.description))
            .bind(("price", e.price_per_hour_cents))
            .bind(("images", e.images))
            .bind(("lng", e.lng))
            .bind(("lat", e.lat))
            .bind(("address", e.address))
            .bind(("availability", e.availability))
            .bind(("timezone", e.timezone))
            .bind(("at", Datetime::from(at)))
            .bind(("seq", seq as i64))
            .await?
            .check()?;
        Ok(())
    }

    async fn spot_updated(
        &self,
        e: SpotUpdated,
        at: chrono::DateTime<chrono::Utc>,
        seq: u64,
    ) -> MyResult<()> {
        self.db
            .query(
                "BEGIN;
                 UPDATE type::record('spot', $id) SET
                     title          = $title        ?? title,
                     description    = $description  ?? description,
                     price_per_hour = $price        ?? price_per_hour,
                     images         = $images       ?? images,
                     availability   = $availability ?? availability,
                     updated_at     = $at;
                 UPSERT _projection:SPOTS SET last_seq = $seq, updated_at = $at;
                 COMMIT;",
            )
            .bind(("id", record_key(&e.spot_id)))
            .bind(("title", e.title))
            .bind(("description", e.description))
            .bind(("price", e.price_per_hour_cents))
            .bind(("images", e.images))
            .bind(("availability", e.availability))
            .bind(("at", Datetime::from(at)))
            .bind(("seq", seq as i64))
            .await?
            .check()?;
        Ok(())
    }

    async fn spot_deactivated(
        &self,
        spot_id: Uuid,
        at: chrono::DateTime<chrono::Utc>,
        seq: u64,
    ) -> MyResult<()> {
        self.db
            .query(
                "BEGIN;
                 UPDATE type::record('spot', $id) SET active = false, updated_at = $at;
                 UPSERT _projection:SPOTS SET last_seq = $seq, updated_at = $at;
                 COMMIT;",
            )
            .bind(("id", record_key(&spot_id)))
            .bind(("at", Datetime::from(at)))
            .bind(("seq", seq as i64))
            .await?
            .check()?;
        Ok(())
    }

    // ─── reads ──────────────────────────────────────────────────────────────

    /// Profile for `GET /api/view/me`. `None` while the user's event is still in
    /// flight — the caller answers with the claim's id regardless, so this never
    /// has to 404.
    pub async fn profile(&self, user_uuid: &str) -> MyResult<Option<crate::route::me::Profile>> {
        let profile: Option<crate::route::me::Profile> = self
            .db
            .query(
                "SELECT first_name, last_name, profile_picture
                 FROM ONLY type::record('user', $id)",
            )
            .bind(("id", user_uuid.to_string()))
            .await?
            .take(0)?;
        Ok(profile)
    }
}
