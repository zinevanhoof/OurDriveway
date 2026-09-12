use chrono::{DateTime, Utc};
use diesel::dsl::sum;
use diesel::prelude::*;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use shared::diesel_ext::to_bigint;
use shared::domain_models::booking::status as booking_status;
use shared::domain_models::payment::status;
use shared::domain_models::payment::{Payment, PaymentPatch};
use shared::error::myerror::MyResult;
use shared::schema::payment::{booking, payment};
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
        conn: &mut AsyncPgConnection,
        booking_id: Uuid,
    ) -> MyResult<Option<Payment>> {
        Ok(payment::table
            .filter(payment::booking_id.eq(booking_id))
            .select(Payment::as_select())
            .first(conn)
            .await
            .optional()?)
    }

    /// `payment_session … UNIQUE`. The checkout screen knows only a session id.
    pub async fn find_by_session_id(
        conn: &mut AsyncPgConnection,
        session_id: String,
    ) -> MyResult<Option<Payment>> {
        Ok(payment::table
            .filter(payment::session_id.eq(session_id))
            .select(Payment::as_select())
            .first(conn)
            .await
            .optional()?)
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
    pub async fn all(conn: &mut AsyncPgConnection) -> MyResult<Vec<Payment>> {
        Ok(payment::table
            .order(payment::created_at.asc())
            .select(Payment::as_select())
            .load(conn)
            .await?)
    }

    /// Insert-or-replace the whole row, keyed by its own id.
    ///
    /// Idempotent by construction, which is what lets a projector replay the same
    /// event.
    pub async fn upsert(conn: &mut AsyncPgConnection, row: Payment) -> MyResult<()> {
        diesel::insert_into(payment::table)
            .values(row.clone())
            .on_conflict(payment::id)
            .do_update()
            .set(row)
            .execute(conn)
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
        conn: &mut AsyncPgConnection,
        payment_id: Uuid,
        from: &[&str],
        patch: PaymentPatch,
    ) -> MyResult<()> {
        diesel::update(payment::table.find(payment_id).filter(
            payment::status.eq_any(from.iter().map(|s| s.to_string()).collect::<Vec<_>>()),
        ))
        .set(&patch)
        .execute(conn)
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
    /// two conditions, one pass, and `booking_host (host_id, status, ends_at)` serves
    /// the inner half.
    ///
    /// `cutoff` is passed in rather than read from a clock here, so this stays a pure
    /// query and the caller owns the window.
    pub async fn earned(
        conn: &mut AsyncPgConnection,
        host_id: &Uuid,
        cutoff: DateTime<Utc>,
    ) -> MyResult<i64> {
        // `sum()` over no rows is NULL — a host who has earned nothing is the ordinary
        // case on a fresh account — so it arrives as `None` and `unwrap_or(0)` is the
        // default. Deliberately not `COALESCE(…, 0)` in SQL, which would flatten "no
        // bookings yet" and "earned nothing" into one value before Rust could tell them
        // apart.
        //
        // `to_bigint` because `sum(bigint)` is numeric in Postgres, which does not decode
        // into an `i64` — Postgres widens to avoid overflow, and cents in an `i64` cannot
        // get near that bound, so casting back is safe. See `shared::diesel_ext`.
        //
        // **`host_id` is asserted on both sides of the join**, which is not redundant:
        // it is what lets the planner start from `booking_host (host_id, status,
        // ends_at)` instead of reaching every booking a payment points at.
        let total: Option<i64> = payment::table
            .inner_join(booking::table.on(booking::id.eq(payment::booking_id)))
            .filter(
                payment::host_id
                    .eq(host_id)
                    .and(payment::status.eq(status::SUCCEEDED))
                    .and(booking::host_id.eq(host_id))
                    .and(booking::status.eq(booking_status::CONFIRMED))
                    .and(booking::ends_at.lt(cutoff)),
            )
            .select(to_bigint(sum(payment::amount_cents)))
            .first(conn)
            .await?;

        Ok(total.unwrap_or(0))
    }
}
