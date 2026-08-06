use chrono::{DateTime, Utc};
use shared::{
    error::myerror::MyResult,
    events::{
        Envelope,
        booking::{BookingEvent, BookingReserved, ReleaseReason},
        spot::{SpotCreated, SpotEvent, SpotUpdated},
        user::record_key,
    },
    general_models::booking::{Booked, BookingRow, fold_booked},
    general_models::spot::Availability,
};
use serde::Deserialize;
use surrealdb::{
    Surreal,
    engine::remote::ws::Client,
    types::{Datetime, SurrealValue},
};
use uuid::Uuid;

/// The **only** writer to this database. Request handlers publish to NATS and
/// return; everything that lands here arrives via a projector.
pub struct BookingRepository {
    pub db: Surreal<Client>,
}

/// Everything reserve needs, in one point read.
#[derive(Debug, Deserialize, SurrealValue)]
pub struct SpotForBooking {
    pub owner_id: Option<String>,
    pub shard: Option<String>,
    pub price_per_hour: Option<i64>,
    pub availability: Option<Availability>,
    pub active: bool,
    pub booked: Booked,
    /// IANA name, e.g. `"Europe/Brussels"`. `booked` is bare wall-clock strings in
    /// this zone, so cancel's deadline can't be placed on a timeline without it.
    pub timezone: Option<String>,
    /// Stream sequence of the last BOOKINGS event applied for this spot — the
    /// value reserve asserts as `Nats-Expected-Last-Subject-Sequence`.
    pub bookings_seq: u64,
}

/// A booking as confirm, release, and reserve's lost-ack recovery need it.
#[derive(Debug, Deserialize, SurrealValue)]
pub struct BookingForUpdate {
    pub spot_id: String,
    pub spot_shard: String,
    pub renter_id: String,
    pub status: String,
    pub booked: Booked,
    pub amount: i64,
    pub hold_until: Option<DateTime<Utc>>,
}

/// A lapsed hold the sweeper is about to release.
#[derive(Debug, Deserialize, SurrealValue)]
pub struct LapsedHold {
    /// Bare uuid, not a `RecordId` — the sweeper parses it straight back into a
    /// `Uuid` for the event, and the table name would only be in the way.
    pub id: String,
    pub spot_id: String,
    pub spot_shard: String,
}

impl BookingRepository {
    pub async fn last_seq(&self, stream: &str) -> MyResult<u64> {
        let seq: Option<i64> = self
            .db
            .query("SELECT VALUE last_seq FROM ONLY type::record('_projection', $s)")
            .bind(("s", stream.to_string()))
            .await?
            .take(0)?;
        Ok(seq.unwrap_or(0).max(0) as u64)
    }

    // ─── reads ──────────────────────────────────────────────────────────────

    pub async fn spot_for_booking(&self, spot_id: &str) -> MyResult<Option<SpotForBooking>> {
        Ok(self
            .db
            .query(
                // `?? {}` / `?? 0` rather than trusting the schema DEFAULTs: a row
                // created by the BOOKINGS-first ordering, or one written before
                // these fields existed, has neither, and NONE won't deserialize.
                "SELECT owner_id, shard, price_per_hour, availability, active, timezone,
                        booked ?? {} AS booked, bookings_seq ?? 0 AS bookings_seq
                 FROM ONLY type::record('spot', $id)",
            )
            .bind(("id", spot_id.to_string()))
            .await?
            .take(0)?)
    }

    pub async fn booking_for_update(&self, booking_id: &str) -> MyResult<Option<BookingForUpdate>> {
        Ok(self
            .db
            .query(
                "SELECT spot_id, spot_shard, renter_id, status, booked, amount, hold_until
                 FROM ONLY type::record('booking', $id)",
            )
            .bind(("id", booking_id.to_string()))
            .await?
            .take(0)?)
    }

    /// Holds whose expiry has passed. The sweeper's entire query.
    pub async fn lapsed_holds(&self, limit: usize) -> MyResult<Vec<LapsedHold>> {
        Ok(self
            .db
            .query(
                "SELECT record::id(id) AS id, spot_id, spot_shard FROM booking
                 WHERE status = 'reserved' AND hold_until < time::now()
                 LIMIT $limit",
            )
            .bind(("limit", limit as i64))
            .await?
            .take(0)?)
    }

    // ─── SPOTS projection ───────────────────────────────────────────────────

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
        at: DateTime<Utc>,
        seq: u64,
    ) -> MyResult<()> {
        // MERGE, never CONTENT. CONTENT replaces the whole record body with the
        // listed keys, which would wipe `booked` and `bookings_seq` — the SPOTS and
        // BOOKINGS projectors advance independently, so on any cold rebuild this
        // event can land after bookings for the same spot have already applied.
        self.db
            .query(
                "BEGIN;
                 UPSERT type::record('spot', $id) MERGE {
                     owner_id: $owner_id, shard: $shard, price_per_hour: $price,
                     availability: $availability, timezone: $timezone, active: true
                 };
                 UPSERT _projection:SPOTS SET last_seq = $seq, updated_at = $at;
                 COMMIT;",
            )
            .bind(("id", record_key(&e.spot_id)))
            .bind(("owner_id", e.owner_id))
            .bind(("shard", e.shard))
            .bind(("price", e.price_per_hour_cents))
            .bind(("availability", e.availability))
            .bind(("timezone", e.timezone))
            .bind(("at", Datetime::from(at)))
            .bind(("seq", seq as i64))
            .await?
            .check()?;
        Ok(())
    }

    async fn spot_updated(&self, e: SpotUpdated, at: DateTime<Utc>, seq: u64) -> MyResult<()> {
        // `??` keeps the existing value when the event field is None: absent means
        // "unchanged", not "clear".
        self.db
            .query(
                "BEGIN;
                 UPSERT type::record('spot', $id) SET
                     price_per_hour = $price        ?? price_per_hour,
                     availability   = $availability ?? availability;
                 UPSERT _projection:SPOTS SET last_seq = $seq, updated_at = $at;
                 COMMIT;",
            )
            .bind(("id", record_key(&e.spot_id)))
            .bind(("price", e.price_per_hour_cents))
            .bind(("availability", e.availability))
            .bind(("at", Datetime::from(at)))
            .bind(("seq", seq as i64))
            .await?
            .check()?;
        Ok(())
    }

    async fn spot_deactivated(&self, spot_id: Uuid, at: DateTime<Utc>, seq: u64) -> MyResult<()> {
        self.db
            .query(
                "BEGIN;
                 UPSERT type::record('spot', $id) SET active = false;
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

    // ─── BOOKINGS projection ────────────────────────────────────────────────

    pub async fn apply_booking(&self, envelope: Envelope<BookingEvent>, seq: u64) -> MyResult<()> {
        let at = envelope.occurred_at;
        match envelope.payload {
            BookingEvent::Reserved(e) => self.reserved(e, at, seq).await,
            BookingEvent::Confirmed { booking_id } => self.confirmed(booking_id, at, seq).await,
            BookingEvent::Released { booking_id, reason } => {
                self.released(booking_id, reason, at, seq).await
            }
            BookingEvent::Cancelled { booking_id } => self.cancelled(booking_id, at, seq).await,
        }
    }

    async fn reserved(&self, e: BookingReserved, at: DateTime<Utc>, seq: u64) -> MyResult<()> {
        let spot_id = record_key(&e.spot_id);
        self.db
            .query(
                "UPSERT type::record('booking', $id) CONTENT {
                     spot_id: $spot_id, spot_shard: $spot_shard, owner_id: $owner_id,
                     renter_id: $renter_id, booked: $booked, amount: $amount,
                     status: 'reserved', hold_until: $expires_at, created_at: $at
                 };",
            )
            .bind(("id", record_key(&e.booking_id)))
            .bind(("spot_id", spot_id.clone()))
            .bind(("spot_shard", e.spot_shard))
            .bind(("owner_id", e.owner_id))
            .bind(("renter_id", e.renter_id))
            .bind(("booked", e.booked))
            .bind(("amount", e.amount_cents))
            .bind(("expires_at", Datetime::from(e.expires_at)))
            .bind(("at", Datetime::from(at)))
            .await?
            .check()?;
        self.refold(&spot_id, at, seq).await
    }

    async fn confirmed(&self, booking_id: Uuid, at: DateTime<Utc>, seq: u64) -> MyResult<()> {
        // Scoped to 'reserved' so a duplicate delivery can't resurrect a booking
        // that was since released.
        let spot_id = self
            .status_transition(booking_id, "confirmed", &["reserved"], None)
            .await?;
        self.refold_opt(spot_id, at, seq).await
    }

    async fn released(
        &self,
        booking_id: Uuid,
        reason: ReleaseReason,
        at: DateTime<Utc>,
        seq: u64,
    ) -> MyResult<()> {
        // Only a *reserved* booking can be released. A payment landing microseconds
        // before the hold lapses, with the sweeper's event arriving second, must not
        // undo the confirmation — this WHERE clause is the whole guard.
        let spot_id = self
            .status_transition(booking_id, "released", &["reserved"], Some(reason.as_str()))
            .await?;
        self.refold_opt(spot_id, at, seq).await
    }

    async fn cancelled(&self, booking_id: Uuid, at: DateTime<Utc>, seq: u64) -> MyResult<()> {
        // Only a *confirmed* booking can be cancelled, so a redelivery after the
        // booking was settled some other way is a no-op. `release_reason` stays
        // NONE: the status already says which of the two happened, and a value on a
        // field named *release*_reason would only repeat it.
        let spot_id = self
            .status_transition(booking_id, "cancelled", &["confirmed"], None)
            .await?;
        self.refold_opt(spot_id, at, seq).await
    }

    /// Moves a booking to `to` only if it is currently in one of `from`, returning
    /// its spot id when the transition applied.
    async fn status_transition(
        &self,
        booking_id: Uuid,
        to: &str,
        from: &[&str],
        reason: Option<&str>,
    ) -> MyResult<Option<String>> {
        let spot_id: Option<String> = self
            .db
            .query(
                "UPDATE type::record('booking', $id) SET
                     status = $to,
                     hold_until = NONE,
                     release_reason = $reason ?? release_reason
                 WHERE status IN $from
                 RETURN VALUE spot_id;",
            )
            .bind(("id", record_key(&booking_id)))
            .bind(("to", to.to_string()))
            .bind(("from", from.iter().map(|s| s.to_string()).collect::<Vec<_>>()))
            .bind(("reason", reason.map(str::to_string)))
            .await?
            .take(0)?;
        Ok(spot_id)
    }

    async fn refold_opt(&self, spot_id: Option<String>, at: DateTime<Utc>, seq: u64) -> MyResult<()> {
        match spot_id {
            Some(id) => self.refold(&id, at, seq).await,
            // The transition didn't apply (already released, already confirmed).
            // `booked` is unchanged, but the cursor still has to advance or this
            // event replays forever.
            None => self.advance(seq, at).await,
        }
    }

    /// Recomputes `spot.booked` from this spot's booking rows and advances both
    /// cursors in one transaction.
    ///
    /// A full recompute, not an incremental merge: re-delivering the same event
    /// then produces the same map, where appending slots would double them up.
    async fn refold(&self, spot_id: &str, at: DateTime<Utc>, seq: u64) -> MyResult<()> {
        let rows: Vec<BookingRow> = self
            .db
            .query("SELECT status, booked FROM booking WHERE spot_id = $spot_id")
            .bind(("spot_id", spot_id.to_string()))
            .await?
            .take(0)?;

        self.db
            .query(
                "BEGIN;
                 UPSERT type::record('spot', $id) SET
                     booked = $booked,
                     -- math::max so a redelivered older message can't rewind the
                     -- CAS cursor, which would make every reserve assert too low.
                     bookings_seq = math::max([bookings_seq ?? 0, $seq]);
                 UPSERT _projection:BOOKINGS SET last_seq = $seq, updated_at = $at;
                 COMMIT;",
            )
            .bind(("id", spot_id.to_string()))
            .bind(("booked", fold_booked(&rows, at)))
            .bind(("at", Datetime::from(at)))
            .bind(("seq", seq as i64))
            .await?
            .check()?;
        Ok(())
    }

    async fn advance(&self, seq: u64, at: DateTime<Utc>) -> MyResult<()> {
        self.db
            .query("UPSERT _projection:BOOKINGS SET last_seq = $seq, updated_at = $at;")
            .bind(("at", Datetime::from(at)))
            .bind(("seq", seq as i64))
            .await?
            .check()?;
        Ok(())
    }
}
