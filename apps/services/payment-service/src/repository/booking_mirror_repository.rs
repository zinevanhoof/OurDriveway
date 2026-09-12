use diesel::prelude::*;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use shared::domain_models::payment::{BookingMirror, BookingMirrorPatch};
use shared::error::myerror::MyResult;
use shared::schema::payment::booking;
use uuid::Uuid;

/// payment-service's local mirror of the `booking` table.
pub struct BookingMirrorRepository;

impl BookingMirrorRepository {
    /// `SELECT *`. The `record::id(id) AS id` and `booked ?? {}` this used to carry
    /// are both gone: the id is a uuid column, and `booked` is
    /// `NOT NULL DEFAULT '{}'`.
    pub async fn find_by_id(
        conn: &mut AsyncPgConnection,
        booking_id: Uuid,
    ) -> MyResult<Option<BookingMirror>> {
        Ok(booking::table
            .find(booking_id)
            .select(BookingMirror::as_select())
            .first(conn)
            .await
            .optional()?)
    }

    /// Insert-or-replace the whole row.
    ///
    /// A whole-row write, not a merge: `BookingCreated` is always the first event for
    /// a booking and BOOKINGS never expires, so this row is only ever created complete
    /// — which is also why nothing on this table is `Option` except the genuinely
    /// optional columns.
    pub async fn upsert(conn: &mut AsyncPgConnection, row: BookingMirror) -> MyResult<()> {
        diesel::insert_into(booking::table)
            .values(row.clone())
            .on_conflict(booking::id)
            .do_update()
            .set(row)
            .execute(conn)
            .await?;
        Ok(())
    }

    /// Patch a booking only if it is currently in one of `from`, and clear the hold.
    ///
    /// Two things worth their own statement, both load-bearing:
    ///
    /// - The `WHERE`. Scoped for the same reason booking-service's own transition is:
    ///   a payment landing microseconds before the hold lapses, with the sweeper's
    ///   event arriving second, must not undo the confirmation.
    /// - `hold_until = NULL`. A patch's `COALESCE` can leave a value alone but never
    ///   clear it, and every transition out of `reserved` ends the hold. Set here
    ///   rather than by each caller, because forgetting it on one arm leaves a settled
    ///   booking that still looks held.
    ///
    /// The `SET` list deliberately omits `hold_until` from the patch, so the
    /// unconditional assignment is the only write to that column and the two cannot
    /// fight. The other three are every column [`BookingMirrorPatch`] carries.
    pub async fn transition(
        conn: &mut AsyncPgConnection,
        booking_id: Uuid,
        from: &[&str],
        patch: BookingMirrorPatch,
    ) -> MyResult<()> {
        diesel::update(booking::table.find(booking_id).filter(
            booking::status.eq_any(from.iter().map(|s| s.to_string()).collect::<Vec<_>>()),
        ))
        // The patch AND an unconditional `hold_until = NULL`, in one `set`.
        //
        // This is the one place `AsChangeset` alone is wrong. It omits an absent field
        // rather than writing it, which matches `COALESCE($n, column)` exactly — but
        // `hold_until` was never a patch field: it is a CLEAR, assigned unconditionally
        // because every transition out of `reserved` ends the hold. Dropping it left a
        // settled booking still carrying an expiry, which
        // `a_booking_round_trips_its_map_and_clears_its_hold_on_transition` caught.
        .set((
            &patch,
            booking::hold_until.eq(None::<chrono::DateTime<chrono::Utc>>),
        ))
        .execute(conn)
        .await?;
        Ok(())
    }
}
