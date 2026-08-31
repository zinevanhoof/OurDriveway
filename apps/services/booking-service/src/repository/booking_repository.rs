use chrono::{DateTime, Utc};
use shared::domain_models::booking::Booking;
use shared::error::myerror::MyResult;
use shared::general_models::booking::Booked;
use sqlx::PgExecutor;
use uuid::Uuid;

/// The `booking` table.
///
/// Six statements: the point read, the write, one conditional transition, and three
/// range scans — what blocks a spot, what has lapsed, and what is still owed on a
/// spot.
pub struct BookingRepository;

impl BookingRepository {
    /// `SELECT *`. `spot_id` stays a plain uuid: the spot table is in another
    /// service's database and could never be dereferenced from here.
    pub async fn find_by_id(
        ex: impl PgExecutor<'_>,
        booking_id: Uuid,
    ) -> MyResult<Option<Booking>> {
        Ok(sqlx::query_as("SELECT * FROM booking WHERE id = $1")
            .bind(booking_id)
            .fetch_optional(ex)
            .await?)
    }

    /// Every booking, for `BookingService::backfill`.
    ///
    /// ponytail: reads the whole table into memory in one pass, and this is the table
    /// most likely to be the one that outgrows it. Page on `id` — `WHERE id > $after
    /// ORDER BY id LIMIT $n` — when it does.
    pub async fn all(ex: impl PgExecutor<'_>) -> MyResult<Vec<Booking>> {
        Ok(sqlx::query_as("SELECT * FROM booking").fetch_all(ex).await?)
    }

    /// Insert-or-replace the whole row, keyed by its own id.
    ///
    /// Idempotent by construction, which is what lets a projector replay the same
    /// event. Every column of this table is owned by the BOOKINGS stream, so there is
    /// nothing for a whole-row write to erase.
    pub async fn upsert(ex: impl PgExecutor<'_>, booking: Booking) -> MyResult<()> {
        sqlx::query(
            "INSERT INTO booking
                 (id, version, spot_id, owner_id, renter_id, booked, amount, status,
                  hold_until, release_reason, cancel_reason, ends_at, rating, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
             ON CONFLICT (id) DO UPDATE SET
                 version        = EXCLUDED.version,
                 spot_id        = EXCLUDED.spot_id,
                 owner_id       = EXCLUDED.owner_id,
                 renter_id      = EXCLUDED.renter_id,
                 booked         = EXCLUDED.booked,
                 amount         = EXCLUDED.amount,
                 status         = EXCLUDED.status,
                 hold_until     = EXCLUDED.hold_until,
                 release_reason = EXCLUDED.release_reason,
                 cancel_reason  = EXCLUDED.cancel_reason,
                 ends_at        = EXCLUDED.ends_at,
                 rating         = EXCLUDED.rating,
                 created_at     = EXCLUDED.created_at",
        )
        .bind(booking.id)
        .bind(booking.version as i64)
        .bind(booking.spot_id)
        .bind(booking.owner_id)
        .bind(booking.renter_id)
        .bind(sqlx::types::Json(booking.booked))
        .bind(booking.amount)
        .bind(booking.status)
        .bind(booking.hold_until)
        .bind(booking.release_reason)
        .bind(booking.cancel_reason)
        .bind(booking.ends_at)
        .bind(booking.rating)
        .bind(booking.created_at)
        .execute(ex)
        .await?;
        Ok(())
    }

    /// Moves a booking to `to` only if it is currently in one of `from`.
    ///
    /// The whole value here is the `WHERE` — a payment landing microseconds before a
    /// hold lapses, with the sweeper's event arriving second, must not undo the
    /// confirmation. Matching nothing is a legitimate no-op, not an error.
    ///
    /// **Returns nothing, and used to return the spot id.** That was a second
    /// statement, deliberately run whether or not the guard applied, because the
    /// caller needed the spot to advance its compare-and-swap cursor — and skipping
    /// the advance on a refused transition would leave the subject head permanently
    /// ahead of the cursor, refusing every later reserve on that spot. The cursor is
    /// gone (see `SpotMirrorRepository`), no caller ever read the value, and the
    /// second statement went with it.
    ///
    /// The two reasons are separate columns, not one: `release_reason` says why a
    /// *hold* ended, `cancel_reason` says who withdrew a *paid* booking. Only ever one
    /// is `Some`, but collapsing them would leave a renter's history unable to tell
    /// "your hold ran out" from "the host pulled the listing".
    pub async fn transition(
        ex: impl PgExecutor<'_>,
        booking_id: Uuid,
        to: &str,
        from: &[&str],
        release: Option<&str>,
        cancel: Option<&str>,
    ) -> MyResult<()> {
        // `= ANY($5)` rather than an `IN` list built by hand: the set is a parameter,
        // so there is no string to assemble and no arity to get wrong.
        sqlx::query(
            "UPDATE booking SET
                 status         = $2,
                 hold_until     = NULL,
                 release_reason = COALESCE($3, release_reason),
                 cancel_reason  = COALESCE($4, cancel_reason)
             WHERE id = $1 AND status = ANY($5)",
        )
        .bind(booking_id)
        .bind(to)
        .bind(release)
        .bind(cancel)
        .bind(from.iter().map(|s| s.to_string()).collect::<Vec<_>>())
        .execute(ex)
        .await?;
        Ok(())
    }

    /// The slots that block a new booking on one spot, as of `now`.
    ///
    /// This is the whole of what `spot.booked` used to be, asked directly. It is also
    /// why there is no denormalized copy any more: the rows *are* the answer, so there
    /// is nothing to keep in step with them.
    ///
    /// `ends_at > $2` is what bounds the scan, and `booking_spot (spot_id, ends_at)`
    /// is what serves it. The old fold had no such floor and read every booking a spot
    /// had ever taken, on every event.
    ///
    /// `status <> ALL` rather than a positive list: a status this build has never
    /// heard of has to block, because the safe direction for availability is keeping a
    /// slot unavailable rather than selling it twice. Carrying no `hold_until` filter
    /// is the same deliberate choice `spot.booked` made — a lapsed hold stops blocking
    /// when the sweeper publishes `Released`, not because this learned to skip it.
    pub async fn taken_for_spot(
        ex: impl PgExecutor<'_>,
        spot_id: &Uuid,
        now: DateTime<Utc>,
    ) -> MyResult<Booked> {
        let maps: Vec<sqlx::types::Json<Booked>> = sqlx::query_scalar(
            "SELECT booked FROM booking
              WHERE spot_id = $1
                AND ends_at > $2
                AND status <> ALL($3)",
        )
        .bind(spot_id)
        .bind(now)
        .bind(vec!["released".to_string(), "cancelled".to_string()])
        .fetch_all(ex)
        .await?;

        // A union, not the old fold: the rows are already filtered to the ones that
        // block, and nothing downstream depends on the order — unlike `spot.booked`,
        // which had to be byte-identical across replicas because it was stored.
        let mut booked = Booked::new();
        for map in maps {
            for (date, slots) in map.0 {
                booked.entry(date).or_default().extend(slots);
            }
        }
        Ok(booked)
    }

    /// Holds whose expiry has passed. The sweeper's entire query, served by
    /// `booking_hold (status, hold_until)`.
    ///
    /// The one clock read in this service that is allowed to be a clock read: the
    /// sweeper is a wall-clock job, not a projection, and its output is an event that
    /// everything downstream derives deterministically.
    pub async fn lapsed_holds(ex: impl PgExecutor<'_>, limit: usize) -> MyResult<Vec<Booking>> {
        Ok(sqlx::query_as(
            "SELECT * FROM booking
              WHERE status = 'reserved' AND hold_until < now()
              LIMIT $1",
        )
        .bind(limit as i64)
        .fetch_all(ex)
        .await?)
    }

    /// Paid bookings on one spot that are still to come, as of `at`.
    ///
    /// `at` is the event's `occurred_at`, not a clock read: a replayed SpotDeleted then
    /// selects the same set it selected the first time, which is what keeps the cancel
    /// reactor's output a function of the log.
    pub async fn upcoming_confirmed(
        ex: impl PgExecutor<'_>,
        spot_id: &Uuid,
        at: DateTime<Utc>,
    ) -> MyResult<Vec<Booking>> {
        Ok(sqlx::query_as(
            "SELECT * FROM booking
              WHERE spot_id = $1 AND status = 'confirmed' AND ends_at > $2",
        )
        .bind(spot_id)
        .bind(at)
        .fetch_all(ex)
        .await?)
    }
}
