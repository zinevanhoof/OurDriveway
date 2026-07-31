use shared::{
    error::myerror::MyResult,
    events::{
        Envelope,
        spot::{SpotCreated, SpotEvent, SpotUpdated},
        user::record_key,
    },
};
use surrealdb::{Surreal, engine::remote::ws::Client};
use uuid::Uuid;

/// The **only** writer to this database. Request handlers publish to NATS and
/// return; everything that lands here arrives via the projector.
pub struct SpotRepository {
    pub db: Surreal<Client>,
}

impl SpotRepository {
    pub async fn last_seq(&self) -> MyResult<u64> {
        let seq: Option<i64> = self
            .db
            .query("SELECT VALUE last_seq FROM ONLY _projection:SPOTS")
            .await?
            .take(0)?;
        Ok(seq.unwrap_or(0).max(0) as u64)
    }

    pub async fn apply(&self, envelope: Envelope<SpotEvent>, seq: u64) -> MyResult<()> {
        // `occurred_at` from the event, never time::now() — every replica must
        // derive the same timestamp from the same event.
        let at = envelope.occurred_at;
        match envelope.payload {
            SpotEvent::Created(e) => self.created(e, at, seq).await,
            SpotEvent::Updated(e) => self.updated(e, at, seq).await,
            SpotEvent::Deactivated { spot_id } => self.deactivated(spot_id, at, seq).await,
        }
    }

    async fn created(
        &self,
        e: SpotCreated,
        at: chrono::DateTime<chrono::Utc>,
        seq: u64,
    ) -> MyResult<()> {
        // UPSERT with an id from the event, not CREATE: replay must be idempotent,
        // and a database-generated id would differ on every replica.
        self.db
            .query(
                "BEGIN;
                 UPSERT type::record('spot', $id) CONTENT {
                     owner_id: $owner_id, shard: $shard, title: $title,
                     description: $description, price_per_hour: $price,
                     images: $images, location: type::point([$lng, $lat]),
                     active: true, address: $address, availability: $availability,
                     timezone: $timezone, created_at: $at, updated_at: $at
                 };
                 UPSERT _projection:SPOTS SET last_seq = $seq, updated_at = $at;
                 COMMIT;",
            )
            .bind(("id", record_key(&e.spot_id)))
            .bind(("owner_id", e.owner_id))
            .bind(("shard", e.shard))
            .bind(("title", e.title))
            .bind(("description", e.description))
            .bind(("price", e.price_per_hour_cents))
            .bind(("images", e.images))
            .bind(("lng", e.lng))
            .bind(("lat", e.lat))
            .bind(("address", e.address))
            .bind(("availability", e.availability))
            .bind(("timezone", e.timezone))
            .bind(("at", surrealdb::types::Datetime::from(at)))
            .bind(("seq", seq as i64))
            .await?
            .check()?;
        Ok(())
    }

    async fn updated(
        &self,
        e: SpotUpdated,
        at: chrono::DateTime<chrono::Utc>,
        seq: u64,
    ) -> MyResult<()> {
        // `??` keeps the existing value when the event field is None: absent means
        // "unchanged", not "clear".
        self.db
            .query(
                "BEGIN;
                 UPDATE type::record('spot', $id) SET
                     title          = $title          ?? title,
                     description    = $description    ?? description,
                     price_per_hour = $price          ?? price_per_hour,
                     images         = $images         ?? images,
                     availability   = $availability   ?? availability,
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
            .bind(("at", surrealdb::types::Datetime::from(at)))
            .bind(("seq", seq as i64))
            .await?
            .check()?;
        Ok(())
    }

    async fn deactivated(
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
            .bind(("at", surrealdb::types::Datetime::from(at)))
            .bind(("seq", seq as i64))
            .await?
            .check()?;
        Ok(())
    }
}
