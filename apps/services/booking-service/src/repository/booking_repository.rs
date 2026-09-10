use chrono::{DateTime, Utc};
use diesel::prelude::*;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use shared::domain_models::booking::Booking;
use shared::error::myerror::MyResult;
use shared::general_models::booking::Booked;
use shared::schema::booking::booking;
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
        conn: &mut AsyncPgConnection,
        booking_id: Uuid,
    ) -> MyResult<Option<Booking>> {
        Ok(booking::table
            .find(booking_id)
            .select(Booking::as_select())
            .first(conn)
            .await
            .optional()?)
    }

    /// Every booking, for `BookingService::backfill`.
    ///
    /// ponytail: reads the whole table into memory in one pass, and this is the table
    /// most likely to be the one that outgrows it. Page on `id` — `WHERE id > $after
    /// ORDER BY id LIMIT $n` — when it does.
    pub async fn all(conn: &mut AsyncPgConnection) -> MyResult<Vec<Booking>> {
        Ok(booking::table
            .select(Booking::as_select())
            .load(conn)
            .await?)
    }

    /// Insert-or-replace the whole row, keyed by its own id.
    ///
    /// Idempotent by construction, which is what lets a projector replay the same
    /// event. Every column of this table is owned by the BOOKINGS stream, so there is
    /// nothing for a whole-row write to erase.
    pub async fn upsert(conn: &mut AsyncPgConnection, row: Booking) -> MyResult<()> {
        diesel::insert_into(booking::table)
            .values(row.clone())
            .on_conflict(booking::id)
            .do_update()
            .set(row)
            .execute(conn)
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
        conn: &mut AsyncPgConnection,
        booking_id: Uuid,
        to: &str,
        from: &[&str],
        release: Option<&str>,
        cancel: Option<&str>,
    ) -> MyResult<()> {
        // `.eq_any` rather than an `IN` list built by hand: the set is a parameter, so
        // there is no string to assemble and no arity to get wrong.
        //
        // `hold_until` is assigned NULL unconditionally — a clear, which an
        // absent-is-unchanged patch cannot express — so this is written as explicit
        // column assignments rather than through an `AsChangeset` struct.
        diesel::update(booking::table.find(booking_id).filter(
            booking::status.eq_any(from.iter().map(|s| s.to_string()).collect::<Vec<_>>()),
        ))
        .set((
            booking::status.eq(to),
            booking::hold_until.eq(None::<DateTime<Utc>>),
            // `None` here means "leave the existing reason alone" rather than "clear
            // it" — only one of the two is ever set. Diesel skips a `None` element of a
            // `set` tuple, so an absent reason is a column the statement never mentions.
            //
            // That is what `COALESCE($n, release_reason)` used to spell out, as a
            // `sql::<Nullable<Text>>` fragment with the column name spliced in as
            // unchecked text. The column reference is a real one now, and the shorter
            // statement is the same rule.
            release.map(|r| booking::release_reason.eq(r)),
            cancel.map(|c| booking::cancel_reason.eq(c)),
        ))
        .execute(conn)
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
        conn: &mut AsyncPgConnection,
        spot_id: &Uuid,
        now: DateTime<Utc>,
    ) -> MyResult<Booked> {
        let maps: Vec<Booked> = booking::table
            .filter(
                booking::spot_id
                    .eq(spot_id)
                    .and(booking::ends_at.gt(now))
                    .and(
                        booking::status
                            .ne_all(vec!["released".to_string(), "cancelled".to_string()]),
                    ),
            )
            .select(booking::booked)
            .load(conn)
            .await?;

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

    /// Holds whose expiry has passed. The sweeper's entire query, served by
    /// `booking_hold (status, hold_until)`.
    ///
    /// The one clock read in this service that is allowed to be a clock read: the
    /// sweeper is a wall-clock job, not a projection, and its output is an event that
    /// everything downstream derives deterministically.
    pub async fn lapsed_holds(
        conn: &mut AsyncPgConnection,
        limit: usize,
    ) -> MyResult<Vec<Booking>> {
        Ok(booking::table
            .filter(
                booking::status
                    .eq("reserved")
                    .and(booking::hold_until.lt(diesel::dsl::now)),
            )
            .limit(limit as i64)
            .select(Booking::as_select())
            .load(conn)
            .await?)
    }

    /// Paid bookings on one spot that are still to come, as of `at`.
    ///
    /// `at` is the event's `occurred_at`, not a clock read: a replayed SpotDeleted then
    /// selects the same set it selected the first time, which is what keeps the cancel
    /// reactor's output a function of the log.
    pub async fn upcoming_confirmed(
        conn: &mut AsyncPgConnection,
        spot_id: &Uuid,
        at: DateTime<Utc>,
    ) -> MyResult<Vec<Booking>> {
        Ok(booking::table
            .filter(
                booking::spot_id
                    .eq(spot_id)
                    .and(booking::status.eq("confirmed"))
                    .and(booking::ends_at.gt(at)),
            )
            .select(Booking::as_select())
            .load(conn)
            .await?)
    }
}
