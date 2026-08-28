use std::sync::Arc;

use chrono::{DateTime, Utc};
use shared::db::Querier;
use shared::domain_models::booking::Booking;
use shared::error::myerror::MyResult;
use shared::general_models::booking::Booked;
use surrealdb::{
    Surreal,
    engine::remote::ws::Client,
    types::{Datetime, vars},
};
use uuid::Uuid;

/// The `booking` table.
///
/// Six statements, each written out where it is issued: the point read, the write,
/// one conditional transition (two statements, one round trip) and three range
/// scans — what blocks a spot, what has lapsed, and what is still owed on a spot.
pub struct BookingRepository<Q: Querier = Arc<Surreal<Client>>> {
    pub q: Q,
}

impl<Q: Querier> BookingRepository<Q> {
    /// `*` takes every column, so a new field on [`Booking`] needs no edit here.
    /// Only `id` is spelled out, because SurrealDB returns it as the record key
    /// `booking:⟨uuid⟩` while the struct holds a plain uuid — and an explicit alias
    /// beats `*` for the same name in either order, checked against 3.2.4 rather
    /// than assumed.
    ///
    /// `spot_id` stays a plain uuid: the spot table is in another service's
    /// database and cannot be dereferenced from here.
    pub async fn find_by_id(&self, booking_id: Uuid) -> MyResult<Option<Booking>> {
        Ok(self
            .q
            .q("SELECT record::id(id) AS id, * FROM ONLY type::record('booking', $v)")
            .bind(("v", booking_id))
            .await?
            .take(0)?)
    }

    /// Every booking, for `BookingService::backfill`.
    ///
    /// ponytail: reads the whole table into memory in one pass, and this is the
    /// table most likely to be the one that outgrows it. Page on `id` — `WHERE id >
    /// $after ORDER BY id LIMIT $n` — when it does.
    pub async fn all(&self) -> MyResult<Vec<Booking>> {
        Ok(self
            .q
            .q("SELECT record::id(id) AS id, * FROM booking")
            .await?
            .take(0)?)
    }

    /// Insert-or-replace the whole row, keyed by its own id.
    ///
    /// Idempotent by construction, which is what lets a projector replay the same
    /// event. `CONTENT $row` binds the struct whole, so adding a field to
    /// [`Booking`] needs no change here.
    ///
    /// The row carries its own `id` and the statement also names one. SurrealDB
    /// requires them to agree and errors if they do not, which makes this a free
    /// assertion rather than a risk.
    ///
    /// Safe as `CONTENT` here, unlike on the spot mirror: every column of this
    /// table is owned by the BOOKINGS stream, so there is nothing for a whole-row
    /// write to erase.
    pub async fn upsert(&self, booking: Booking) -> MyResult<()> {
        let id = booking.id;
        self.q
            .q("UPSERT type::record('booking', $id) CONTENT $row")
            .bind(("id", id))
            .bind(("row", booking))
            .await?
            .check()?;
        Ok(())
    }

    /// Moves a booking to `to` only if it is currently in one of `from`, returning
    /// its spot id.
    ///
    /// The whole value here is the `WHERE` — a payment landing microseconds before
    /// a hold lapses, with the sweeper's event arriving second, must not undo the
    /// confirmation.
    ///
    /// The spot id comes back **whether or not the guard applied**, which is why it
    /// is a second statement rather than a `RETURN VALUE` on the update. The caller
    /// advances that spot's compare-and-swap cursor with it, and a refused
    /// transition still consumed a subject sequence: skipping the advance leaves the
    /// subject head permanently ahead of the cursor, and every later reserve on the
    /// spot asserts a stale sequence and is refused forever. `None` now means the
    /// booking row does not exist — the only case with genuinely nothing to advance.
    ///
    /// The two reasons are separate fields, not one: `release_reason` says why a
    /// *hold* ended, `cancel_reason` says who withdrew a *paid* booking. Only ever
    /// one is Some, but collapsing them would leave a renter's history unable to
    /// tell "your hold ran out" from "the host pulled the listing".
    pub async fn transition(
        &self,
        booking_id: Uuid,
        to: &str,
        from: &[&str],
        release: Option<&str>,
        cancel: Option<&str>,
    ) -> MyResult<Option<Uuid>> {
        Ok(self
            .q
            .q("UPDATE type::record('booking', $id) SET
                    status = $to,
                    hold_until = NONE,
                    release_reason = $release ?? release_reason,
                    cancel_reason  = $cancel  ?? cancel_reason
                WHERE status IN $from;
                SELECT VALUE spot_id FROM ONLY type::record('booking', $id);")
            .bind(vars! {
                id:      booking_id,
                to:      to.to_string(),
                from:    from.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
                release: release.map(str::to_string),
                cancel:  cancel.map(str::to_string),
            })
            .await?
            .take(1)?)
    }

    /// The slots that block a new booking on one spot, as of `now`.
    ///
    /// This is the whole of what `spot.booked` used to be, asked directly. It is
    /// also why there is no denormalized copy any more: the rows *are* the answer,
    /// so there is nothing to keep in step with them.
    ///
    /// `ends_at > $now` is what bounds the scan. The old fold had no such floor and
    /// read every booking a spot had ever taken, on every event.
    ///
    /// `status NOT IN` rather than `IN`: a status this build has never heard of has
    /// to block, because the safe direction for availability is keeping a slot
    /// unavailable rather than selling it twice. Carrying no `hold_until` filter is
    /// the same deliberate choice `spot.booked` made — a lapsed hold stops blocking
    /// when the sweeper publishes `Released`, not because this learned to skip it.
    pub async fn taken_for_spot(&self, spot_id: &Uuid, now: DateTime<Utc>) -> MyResult<Booked> {
        let maps: Vec<Booked> = self
            .q
            .q("SELECT VALUE booked FROM booking
                WHERE spot_id = $spot_id
                  AND ends_at > $now
                  AND status NOT IN ['released', 'cancelled']")
            .bind(vars! {
                spot_id: *spot_id,
                now:     Datetime::from(now),
            })
            .await?
            .take(0)?;

        // A union, not the old fold: the rows are already filtered to the ones that
        // block, and nothing downstream depends on the order — unlike `spot.booked`,
        // which had to be byte-identical across replicas because it was stored.
        let mut booked = Booked::new();
        for map in maps {
            for (date, slots) in map {
                booked.entry(date).or_default().extend(slots);
            }
        }
        Ok(booked)
    }

    /// Holds whose expiry has passed. The sweeper's entire query.
    ///
    /// The one clock read in this service that is allowed to be a clock read: the
    /// sweeper is a wall-clock job, not a projection, and its output is an event
    /// that everything downstream derives deterministically.
    pub async fn lapsed_holds(&self, limit: usize) -> MyResult<Vec<Booking>> {
        Ok(self
            .q
            .q("SELECT record::id(id) AS id, * FROM booking
                WHERE status = 'reserved' AND hold_until < time::now()
                LIMIT $limit")
            .bind(("limit", limit as i64))
            .await?
            .take(0)?)
    }

    /// Paid bookings on one spot that are still to come, as of `at`.
    ///
    /// `at` is the event's `occurred_at`, not a clock read: a replayed SpotDeleted
    /// then selects the same set it selected the first time, which is what keeps
    /// the cancel reactor's output a function of the log.
    pub async fn upcoming_confirmed(
        &self,
        spot_id: &Uuid,
        at: DateTime<Utc>,
    ) -> MyResult<Vec<Booking>> {
        Ok(self
            .q
            .q("SELECT record::id(id) AS id, * FROM booking
                WHERE spot_id = $spot_id AND status = 'confirmed' AND ends_at > $at")
            .bind(("spot_id", *spot_id))
            .bind(("at", Datetime::from(at)))
            .await?
            .take(0)?)
    }
}
