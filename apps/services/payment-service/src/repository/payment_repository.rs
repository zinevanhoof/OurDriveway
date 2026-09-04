use chrono::{DateTime, Utc};
use shared::domain_models::payment::{Payment, PaymentPatch};
use shared::error::myerror::MyResult;
use sqlx::PgExecutor;
use uuid::Uuid;

/// The `payment` table.
///
/// Five statements: a lookup by each of the two UNIQUE columns, the write, the
/// conditional transition, and the earnings sum.
pub struct PaymentRepository;

impl PaymentRepository {
    /// `payment_booking … UNIQUE`, so at most one row can match — a fact about the
    /// schema rather than a hope about the data.
    pub async fn find_by_booking_id(
        ex: impl PgExecutor<'_>,
        booking_id: Uuid,
    ) -> MyResult<Option<Payment>> {
        Ok(
            sqlx::query_as("SELECT * FROM payment WHERE booking_id = $1")
                .bind(booking_id)
                .fetch_optional(ex)
                .await?,
        )
    }

    /// `payment_session … UNIQUE`. The checkout screen knows only a session id.
    pub async fn find_by_session_id(
        ex: impl PgExecutor<'_>,
        session_id: String,
    ) -> MyResult<Option<Payment>> {
        Ok(
            sqlx::query_as("SELECT * FROM payment WHERE session_id = $1")
                .bind(session_id)
                .fetch_optional(ex)
                .await?,
        )
    }

    /// Every payment, for `PaymentService::backfill`.
    ///
    /// Ordered by `created_at` so a rebuild replays a payment's history in the order it
    /// happened. Not required for correctness — each payment is its own aggregate and
    /// its two backfilled events are enqueued together — but a log that reads
    /// chronologically is worth the `ORDER BY`.
    ///
    /// ponytail: whole table in one pass, same ceiling and same fix as the others —
    /// keyset on `created_at` if this ever has to run against a table that does not fit
    /// in memory.
    pub async fn all(ex: impl PgExecutor<'_>) -> MyResult<Vec<Payment>> {
        Ok(sqlx::query_as("SELECT * FROM payment ORDER BY created_at")
            .fetch_all(ex)
            .await?)
    }

    /// Insert-or-replace the whole row, keyed by its own id.
    ///
    /// Idempotent by construction, which is what lets a projector replay the same
    /// event.
    pub async fn upsert(ex: impl PgExecutor<'_>, payment: Payment) -> MyResult<()> {
        sqlx::query(
            "INSERT INTO payment
                 (id, version, booking_id, owner_id, renter_id, amount_cents,
                  session_id, intent_id, status, refund_id, refunded_at,
                  failure_reason, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
             ON CONFLICT (id) DO UPDATE SET
                 version        = EXCLUDED.version,
                 booking_id     = EXCLUDED.booking_id,
                 owner_id       = EXCLUDED.owner_id,
                 renter_id      = EXCLUDED.renter_id,
                 amount_cents   = EXCLUDED.amount_cents,
                 session_id     = EXCLUDED.session_id,
                 intent_id      = EXCLUDED.intent_id,
                 status         = EXCLUDED.status,
                 refund_id      = EXCLUDED.refund_id,
                 refunded_at    = EXCLUDED.refunded_at,
                 failure_reason = EXCLUDED.failure_reason,
                 created_at     = EXCLUDED.created_at",
        )
        .bind(payment.id)
        .bind(payment.version as i64)
        .bind(payment.booking_id)
        .bind(payment.owner_id)
        .bind(payment.renter_id)
        .bind(payment.amount_cents)
        .bind(payment.session_id)
        .bind(payment.intent_id)
        .bind(payment.status)
        .bind(payment.refund_id)
        .bind(payment.refunded_at)
        .bind(payment.failure_reason)
        .bind(payment.created_at)
        .execute(ex)
        .await?;
        Ok(())
    }

    /// Patch a payment only if it is currently in one of `from`.
    ///
    /// The whole value is the `WHERE`. Money states only ever move forwards, which is
    /// what makes a redelivered event a no-op instead of, say, un-refunding a payment.
    ///
    /// `COALESCE($n, column)` is absent-is-unchanged, and is also the ceiling: no
    /// patch can set a column back to NULL.
    ///
    /// **The binds are positional**, so their order must match the `$n`. Four of the
    /// five are `Option<String>`, so a swapped pair compiles and writes the wrong
    /// column — `set_covers_every_patchable_column` in the model is the reminder to
    /// come here, and the live round-trip is what would catch it.
    pub async fn transition(
        ex: impl PgExecutor<'_>,
        payment_id: Uuid,
        from: &[&str],
        patch: PaymentPatch,
    ) -> MyResult<()> {
        sqlx::query(
            "UPDATE payment SET
                 status         = COALESCE($3, status),
                 intent_id      = COALESCE($4, intent_id),
                 refund_id      = COALESCE($5, refund_id),
                 refunded_at    = COALESCE($6, refunded_at),
                 failure_reason = COALESCE($7, failure_reason)
             WHERE id = $1 AND status = ANY($2)",
        )
        .bind(payment_id)
        .bind(from.iter().map(|s| s.to_string()).collect::<Vec<_>>())
        .bind(patch.status)
        .bind(patch.intent_id)
        .bind(patch.refund_id)
        .bind(patch.refunded_at)
        .bind(patch.failure_reason)
        .execute(ex)
        .await?;
        Ok(())
    }

    /// A host's settled income: paid, not refunded, and for a booking that has
    /// actually happened.
    ///
    /// Two conditions and both are needed. The payment must have succeeded; the
    /// *booking* must still be confirmed and old enough to have settled. The booking
    /// half is what stops a host withdrawing money for a booking that has not happened
    /// yet — see `SETTLEMENT_SECS`.
    ///
    /// A join rather than the nested `booking_id IN (SELECT …)` this replaced. Same
    /// two conditions, one pass, and `booking_owner (owner_id, status, ends_at)` serves
    /// the inner half.
    ///
    /// `cutoff` is passed in rather than read from a clock here, so this stays a pure
    /// query and the caller owns the window.
    pub async fn earned(
        ex: impl PgExecutor<'_>,
        owner_id: &Uuid,
        cutoff: DateTime<Utc>,
    ) -> MyResult<i64> {
        // Two things in that one expression, and both are needed:
        //
        //   COALESCE  SUM over no rows is NULL, not 0 — a host who has earned nothing
        //             is the ordinary case on a fresh account.
        //   ::bigint  `SUM(bigint)` returns **numeric**, which sqlx will not decode
        //             into an i64. Postgres widens to avoid overflow; cents in an i64
        //             cannot get near it, so casting back is safe.
        Ok(sqlx::query_scalar(
            "SELECT COALESCE(SUM(p.amount_cents), 0)::bigint
               FROM payment p
               JOIN booking b ON b.id = p.booking_id
              WHERE p.owner_id = $1
                AND p.status = 'succeeded'
                AND b.owner_id = $1
                AND b.status = 'confirmed'
                AND b.ends_at < $2",
        )
        .bind(owner_id)
        .bind(cutoff)
        .fetch_one(ex)
        .await?)
    }
}
