use chrono::{DateTime, Utc};
use shared::error::myerror::MyResult;
use shared::projections::wallet::{Balance, WalletTransaction};
use sqlx::PgExecutor;
use uuid::Uuid;

/// Everything that moved money for one person, across `payment` and `payout`.
///
/// The only read in this service that spans several tables as *peers* rather than as a
/// row and its joins, which is why it is its own file instead of living on either of
/// them.
///
/// ## Where the security lives
///
/// Both statements are scoped to one caller and there is no public audience for either:
/// a wallet belongs to exactly one person, so the predicate is the whole query rather
/// than a clause appended to it. Every branch of the union names `$1`, and `$1` comes
/// from the verified claim.
pub struct WalletRepository;

impl WalletRepository {
    /// One month of the caller's history, newest first.
    ///
    /// ## Four branches, not one `OR`
    ///
    /// The obvious statement is one scan of `payment` with
    /// `WHERE owner_id = $1 OR renter_id = $1`. It is written as a `UNION ALL` instead
    /// for two reasons, and the second is the one that matters:
    ///
    /// - An `OR` across two columns cannot use either of `payment_owner_created` or
    ///   `payment_renter_created` on its own.
    /// - **The sign differs.** The same row is `+amount` to the host and `-amount` to
    ///   the renter, so the two roles were never going to be one branch — a `CASE` over
    ///   `$1` would be the same four branches wearing a disguise.
    ///
    /// A refund is its own row rather than a mutation of the charge's: the charge
    /// happened, then the money came back, and a history that shows only the second is
    /// missing an event that occurred. They are told apart by the `:kind` suffix on the
    /// id, because both come from one `payment` row.
    ///
    /// `status IN ('succeeded','refunded')` is what keeps the *unpaid* out — a session
    /// that was created and never paid, declined, or expired moved no money and has no
    /// place in a wallet.
    ///
    /// Refunds older than payment-service's `refunded_at` column have no instant to file
    /// under, so `refunded_at IS NOT NULL` quietly leaves them out rather than inventing
    /// a date for them. There are none in any environment that matters.
    ///
    /// ponytail: a month is one page. Ten thousand rows in one month would be fetched
    /// whole; the fix is keyset pagination on `(occurred_at, id)` *within* a month, and
    /// the totals then have to become aggregates rather than a fold over what came back.
    pub async fn find_month(
        ex: impl PgExecutor<'_>,
        caller_id: Uuid,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        settled_before: DateTime<Utc>,
    ) -> MyResult<Vec<WalletTransaction>> {
        Ok(sqlx::query_as(
            "SELECT p.id::text || ':in' AS id,
                    'in'                AS kind,
                    p.amount            AS amount_cents,
                    p.created_at        AS occurred_at,
                    (p.status = 'succeeded' AND b.status = 'confirmed'
                     AND b.ends_at >= $4) AS pending,
                    s.title             AS title,
                    b.booked            AS booked,
                    s.timezone          AS timezone
               FROM payment p
               JOIN booking b ON b.id = p.booking_id
               LEFT JOIN spot s ON s.id = b.spot_id
              WHERE p.owner_id = $1
                AND p.status IN ('succeeded', 'refunded')
                AND p.created_at >= $2 AND p.created_at < $3

             UNION ALL

             SELECT p.id::text || ':out', 'out', -p.amount, p.created_at, false,
                    s.title, b.booked, s.timezone
               FROM payment p
               JOIN booking b ON b.id = p.booking_id
               LEFT JOIN spot s ON s.id = b.spot_id
              WHERE p.renter_id = $1
                AND p.status IN ('succeeded', 'refunded')
                AND p.created_at >= $2 AND p.created_at < $3

             UNION ALL

             SELECT p.id::text || ':refund-out', 'refund', -p.amount, p.refunded_at, false,
                    s.title, b.booked, s.timezone
               FROM payment p
               JOIN booking b ON b.id = p.booking_id
               LEFT JOIN spot s ON s.id = b.spot_id
              WHERE p.owner_id = $1
                AND p.refunded_at IS NOT NULL
                AND p.refunded_at >= $2 AND p.refunded_at < $3

             UNION ALL

             SELECT p.id::text || ':refund-in', 'refund', p.amount, p.refunded_at, false,
                    s.title, b.booked, s.timezone
               FROM payment p
               JOIN booking b ON b.id = p.booking_id
               LEFT JOIN spot s ON s.id = b.spot_id
              WHERE p.renter_id = $1
                AND p.refunded_at IS NOT NULL
                AND p.refunded_at >= $2 AND p.refunded_at < $3

             UNION ALL

             -- `pending` is the transfer still being in flight, which is the same
             -- PENDING chip a not-yet-settled charge gets. `failed` rows are excluded
             -- entirely: the money never left, `balance` does not count them either,
             -- and a row saying a withdrawal happened would be a lie.
             --
             -- ponytail: a failed withdrawal vanishes silently. If a host ever needs
             -- telling, the fix is a `failed` kind in `projections::wallet` and a row
             -- style for it — this predicate becomes `<> 'failed'`-less and the `kind`
             -- expression grows a CASE.
             SELECT po.id::text || ':payout', 'payout', -po.amount, po.created_at,
                    po.status = 'requested',
                    NULL::text, NULL::jsonb, NULL::text
               FROM payout po
              WHERE po.owner_id = $1
                AND po.status <> 'failed'
                AND po.created_at >= $2 AND po.created_at < $3

              ORDER BY occurred_at DESC",
        )
        .bind(caller_id)
        .bind(start)
        .bind(end)
        .bind(settled_before)
        .fetch_all(ex)
        .await?)
    }

    /// The most recent month before `start` that holds anything, as `"YYYY-MM"`.
    ///
    /// This is the cursor. Without it the client would have to guess: ask for the
    /// previous month, get nothing, and either stop — hiding everything behind a quiet
    /// month — or keep walking backwards forever through a history that ended in 2024.
    ///
    /// Four `max()`es over four indexed predicates rather than the union above run a
    /// second time. Each one is an index lookup; none of them reads a row it does not
    /// need. `NULL` — meaning nothing older exists — is the end of the list.
    pub async fn find_previous_month(
        ex: impl PgExecutor<'_>,
        caller_id: Uuid,
        start: DateTime<Utc>,
    ) -> MyResult<Option<String>> {
        Ok(sqlx::query_scalar(
            "SELECT to_char(max(t), 'YYYY-MM')
               FROM (
                 SELECT max(created_at) AS t FROM payment
                  WHERE owner_id = $1 AND status IN ('succeeded', 'refunded')
                    AND created_at < $2
                 UNION ALL
                 SELECT max(created_at) FROM payment
                  WHERE renter_id = $1 AND status IN ('succeeded', 'refunded')
                    AND created_at < $2
                 UNION ALL
                 SELECT max(refunded_at) FROM payment
                  WHERE (owner_id = $1 OR renter_id = $1) AND refunded_at < $2
                 UNION ALL
                 -- Same filter as the union above, or the cursor names a month whose
                 -- only row is a failed withdrawal — and the client asks for a page
                 -- that renders empty.
                 SELECT max(created_at) FROM payout
                  WHERE owner_id = $1 AND status <> 'failed' AND created_at < $2
               ) newest",
        )
        .bind(caller_id)
        .bind(start)
        .fetch_one(ex)
        .await?)
    }

    /// A host's money: what they have earned, what they have taken out, and what is
    /// still ripening.
    ///
    /// **This is payment-service's `earnings` query, moved.** It is the same rule read
    /// from the other side of the split: a payment that succeeded, on a booking that is
    /// still confirmed, that has been over long enough to settle. `settled_before` is
    /// `now - SETTLEMENT_SECS` and must be computed from the same figure payment-service
    /// uses, or a host is shown one balance and withdraws another.
    ///
    /// Nothing is stored. `available = earned - paid_out` on every read is what makes a
    /// refunded booking drop out of a host's income with no compensating write — the
    /// booking stops being `confirmed`, so it simply stops matching.
    ///
    /// The authority on what a withdrawal actually pays out is still
    /// `PaymentService::request_payout`, which computes it inside its own transaction
    /// under an advisory lock. This figure is for display, and it may be behind by
    /// however far the projector is.
    ///
    /// `&mut PgConnection` and not `impl PgExecutor<'_>`, for the same reason
    /// `PaymentService::earnings_with` takes one: this issues two statements, an
    /// executor is consumed per statement, and the two halves of one balance should not
    /// come back from two different pooled connections.
    pub async fn balance(
        conn: &mut sqlx::PgConnection,
        owner_id: Uuid,
        settled_before: DateTime<Utc>,
    ) -> MyResult<Balance> {
        // COALESCE because SUM over no rows is NULL, and a host who has earned nothing
        // is the ordinary case. ::bigint because SUM(bigint) is numeric, which sqlx
        // will not decode into an i64.
        //
        // FILTER rather than three statements: one pass over one index answers all of
        // it, and the two halves of "earned" cannot disagree about where the cutoff is.
        let (earned, pending): (i64, i64) = sqlx::query_as(
            "SELECT COALESCE(SUM(p.amount) FILTER (WHERE b.ends_at <  $2), 0)::bigint,
                    COALESCE(SUM(p.amount) FILTER (WHERE b.ends_at >= $2), 0)::bigint
               FROM payment p
               JOIN booking b ON b.id = p.booking_id
              WHERE p.owner_id = $1
                AND p.status = 'succeeded'
                AND b.status = 'confirmed'",
        )
        .bind(owner_id)
        .bind(settled_before)
        .fetch_one(&mut *conn)
        .await?;

        // `status <> 'failed'` — the same set as `PayoutRepository::total_for` in
        // payment-service, which is the authority. A withdrawal in flight still counts
        // (the host cannot have that money again); a refused one does not, and that is
        // the whole of how the money comes back. These two sums are the same arithmetic
        // over two databases: change one and change the other, or the figure beside the
        // withdraw button stops matching what the button will pay.
        let paid_out: i64 = sqlx::query_scalar(
            "SELECT COALESCE(SUM(amount), 0)::bigint
               FROM payout
              WHERE owner_id = $1 AND status <> 'failed'",
        )
        .bind(owner_id)
        .fetch_one(&mut *conn)
        .await?;

        Ok(Balance {
            available_cents: earned - paid_out,
            earned_cents: earned,
            paid_out_cents: paid_out,
            pending_cents: pending,
        })
    }
}
