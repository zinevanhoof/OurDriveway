use shared::domain_models::payment::{BookingMirror, BookingMirrorPatch};
use shared::error::myerror::MyResult;
use sqlx::PgExecutor;
use uuid::Uuid;

/// payment-service's local mirror of the `booking` table.
pub struct BookingMirrorRepository;

impl BookingMirrorRepository {
    /// `SELECT *`. The `record::id(id) AS id` and `booked ?? {}` this used to carry
    /// are both gone: the id is a uuid column, and `booked` is
    /// `NOT NULL DEFAULT '{}'`.
    pub async fn find_by_id(
        ex: impl PgExecutor<'_>,
        booking_id: Uuid,
    ) -> MyResult<Option<BookingMirror>> {
        Ok(sqlx::query_as("SELECT * FROM booking WHERE id = $1")
            .bind(booking_id)
            .fetch_optional(ex)
            .await?)
    }

    /// Insert-or-replace the whole row.
    ///
    /// A whole-row write, not a merge: `BookingCreated` is always the first event for
    /// a booking and BOOKINGS never expires, so this row is only ever created complete
    /// — which is also why nothing on this table is `Option` except the genuinely
    /// optional columns.
    pub async fn upsert(ex: impl PgExecutor<'_>, booking: BookingMirror) -> MyResult<()> {
        sqlx::query(
            "INSERT INTO booking
                 (id, spot_id, owner_id, renter_id, amount_cents, booked, status,
                  hold_until, ends_at, cancel_reason, release_reason)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
             ON CONFLICT (id) DO UPDATE SET
                 spot_id        = EXCLUDED.spot_id,
                 owner_id       = EXCLUDED.owner_id,
                 renter_id      = EXCLUDED.renter_id,
                 amount_cents   = EXCLUDED.amount_cents,
                 booked         = EXCLUDED.booked,
                 status         = EXCLUDED.status,
                 hold_until     = EXCLUDED.hold_until,
                 ends_at        = EXCLUDED.ends_at,
                 cancel_reason  = EXCLUDED.cancel_reason,
                 release_reason = EXCLUDED.release_reason",
        )
        .bind(booking.id)
        .bind(booking.spot_id)
        .bind(booking.owner_id)
        .bind(booking.renter_id)
        .bind(booking.amount_cents)
        .bind(sqlx::types::Json(booking.booked))
        .bind(booking.status)
        .bind(booking.hold_until)
        .bind(booking.ends_at)
        .bind(booking.cancel_reason)
        .bind(booking.release_reason)
        .execute(ex)
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
        ex: impl PgExecutor<'_>,
        booking_id: Uuid,
        from: &[&str],
        patch: BookingMirrorPatch,
    ) -> MyResult<()> {
        sqlx::query(
            "UPDATE booking SET
                 status         = COALESCE($3, status),
                 cancel_reason  = COALESCE($4, cancel_reason),
                 release_reason = COALESCE($5, release_reason),
                 hold_until     = NULL
             WHERE id = $1 AND status = ANY($2)",
        )
        .bind(booking_id)
        .bind(from.iter().map(|s| s.to_string()).collect::<Vec<_>>())
        .bind(patch.status)
        .bind(patch.cancel_reason)
        .bind(patch.release_reason)
        .execute(ex)
        .await?;
        Ok(())
    }
}
