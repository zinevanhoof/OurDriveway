use chrono::{DateTime, Utc};
use diesel::IntoSql;
use diesel::dsl::{case_when, max, sum};
use diesel::prelude::*;
use diesel::sql_types::{Bool, Jsonb, Nullable, Text, Timestamptz};
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use shared::diesel_ext::to_bigint;
use shared::error::myerror::MyResult;
use shared::projections::wallet::{BalanceProjection, WalletTransactionProjection, kind};
use shared::schema::view::{booking, payment, payout, spot};
use uuid::Uuid;

/// A typed SQL `NULL` for `settles_at`, on every branch that cannot ripen.
///
/// Spelled once because all five branches of the union must agree on the column's type to
/// the letter, and `None::<DateTime<Utc>>.into_sql::<Nullable<Timestamptz>>()` written out
/// five times is five chances to write it differently.
fn no_instant() -> diesel::dsl::AsExprOf<Option<DateTime<Utc>>, Nullable<Timestamptz>> {
    None::<DateTime<Utc>>.into_sql::<Nullable<Timestamptz>>()
}

/// Everything that moved money for one person, across `payment` and `payout`.
///
/// The only read in this service that spans several tables as *peers* rather than as a
/// row and its joins, which is why it is its own file instead of living on either of
/// them.
///
/// ## Where the security lives
///
/// Every read here is scoped to one caller and there is no public audience for any of
/// them: a wallet belongs to exactly one person, so the predicate is the whole query
/// rather than a clause appended to it. Every branch of the union names `$1`, and `$1`
/// comes from the verified claim.
///
/// ## No `sql_query` here at all
///
/// All three reads were strings, each for a reason that did not survive being checked.
/// `FILTER (WHERE …)` is `.aggregate_filter()`, native in diesel 2.3. `SUM(bigint)` is
/// `numeric`, which no cast in diesel's allowlist reaches — one declared [`to_bigint`]
/// does. And both unions needed a string only for something that was never the database's
/// job: an outer `max()` over four branches is the largest of four `Option`s, and
/// `ORDER BY` across a union is a sort of rows already in memory.
///
/// What the DSL buys here is not brevity — [`find_month_for_account`] is longer than the
/// SQL it replaced. It is that all five branches of that union must now agree on nine
/// column types **to the letter**, checked at compile time. A column added to one branch
/// and forgotten in the other four used to be a runtime decode error in production; it is
/// now a build failure.
///
/// [`balance_for_host`]: WalletRepository::balance_for_host
/// [`find_newest_before`]: WalletRepository::find_newest_before
/// [`find_month_for_account`]: WalletRepository::find_month_for_account
pub struct WalletRepository;

impl WalletRepository {
    /// One month of the caller's history, newest first.
    ///
    /// ## Five branches, not one `OR`
    ///
    /// The obvious statement is one scan of `payment` with
    /// `WHERE host_id = $1 OR renter_id = $1`. It is a `UNION ALL` instead for two
    /// reasons, and the second is the one that matters:
    ///
    /// - An `OR` across two columns cannot use either of `payment_host_created` or
    ///   `payment_renter_created` on its own.
    /// - **The sign differs.** The same row is `+amount` to the host and `-amount` to the
    ///   renter, so the two roles were never going to be one branch — a `CASE` over `$1`
    ///   would be the same branches wearing a disguise.
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
    /// under, so a null `refunded_at` quietly leaves them out rather than inventing a date
    /// for them. There are none in any environment that matters.
    ///
    /// ## Ordering and the totals happen in Rust
    ///
    /// `ORDER BY` across a `UNION ALL` has to name a column by *position*, and diesel's
    /// `positional_order_by` is explicitly not public API — "may change without a major
    /// version bump", with unchecked arguments. A month's rows are already in memory by
    /// then, so they are sorted here. `policy::wallet::totals` folds the same `Vec` for
    /// the same reason.
    ///
    /// ponytail: a month is one page. Ten thousand rows in one month would be fetched
    /// whole; the fix is keyset pagination on `(occurred_at, id)` *within* a month, and
    /// the totals then have to become aggregates rather than a fold over what came back.
    pub async fn find_month_for_account(
        conn: &mut AsyncPgConnection,
        caller_id: Uuid,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> MyResult<Vec<WalletTransactionProjection>> {
        // The three columns every payment branch reads off its joins. `spot` is a LEFT
        // join, so its two are already nullable; `booked` is `NOT NULL` on `booking` and
        // has to be widened by hand, because the payout branch has no booking to read.
        let (title, booked, timezone) = (
            spot::title.nullable(),
            booking::booked.nullable(),
            spot::timezone.nullable(),
        );

        // Charged, in the window, and the booking behind it exists. Shared by the two
        // role branches, which differ only in which column is the caller and in the sign.
        let charged = payment::table
            .inner_join(booking::table.on(booking::id.eq(payment::booking_id)))
            .left_join(spot::table.on(spot::id.eq(booking::spot_id)))
            .filter(
                payment::status
                    .eq_any(["succeeded", "refunded"])
                    .and(payment::created_at.ge(start))
                    .and(payment::created_at.lt(end)),
            );

        // Refunded in the window. `refunded_at >= start` implies `IS NOT NULL`, which is
        // what `assume_not_null` below leans on — the column is nullable and this row's
        // value cannot be.
        let refunded = payment::table
            .inner_join(booking::table.on(booking::id.eq(payment::booking_id)))
            .left_join(spot::table.on(spot::id.eq(booking::spot_id)))
            .filter(
                payment::refunded_at
                    .ge(start)
                    .and(payment::refunded_at.lt(end)),
            );

        // A charge on the caller's own spot. The only kind that can still ripen, so the
        // only one with a `settles_at`.
        let money_in = charged
            .clone()
            .filter(payment::host_id.eq(caller_id))
            .select((
                payment::id.cast::<Text>().concat(":in"),
                kind::IN.into_sql::<Text>(),
                payment::amount,
                payment::created_at,
                case_when(
                    payment::status
                        .eq("succeeded")
                        .and(booking::status.eq("confirmed")),
                    booking::ends_at,
                ),
                false.into_sql::<Bool>(),
                title,
                booked,
                timezone,
            ));

        // The same charge seen by the renter who paid it.
        let money_out = charged.filter(payment::renter_id.eq(caller_id)).select((
            payment::id.cast::<Text>().concat(":out"),
            kind::OUT.into_sql::<Text>(),
            payment::amount * -1,
            payment::created_at,
            no_instant(),
            false.into_sql::<Bool>(),
            title,
            booked,
            timezone,
        ));

        // Money leaving the host it had been credited to…
        let refund_out = refunded
            .clone()
            .filter(payment::host_id.eq(caller_id))
            .select((
                payment::id.cast::<Text>().concat(":refund-out"),
                kind::REFUND.into_sql::<Text>(),
                payment::amount * -1,
                payment::refunded_at.assume_not_null(),
                no_instant(),
                false.into_sql::<Bool>(),
                title,
                booked,
                timezone,
            ));

        // …and arriving back with the renter. One `payment` row, two entries, told apart
        // by the id suffix.
        let refund_in = refunded.filter(payment::renter_id.eq(caller_id)).select((
            payment::id.cast::<Text>().concat(":refund-in"),
            kind::REFUND.into_sql::<Text>(),
            payment::amount,
            payment::refunded_at.assume_not_null(),
            no_instant(),
            false.into_sql::<Bool>(),
            title,
            booked,
            timezone,
        ));

        // `pending_now` is the transfer still being in flight, which earns the same
        // PENDING chip a not-yet-settled charge gets — but off a status rather than a
        // deadline, which is why it is its own column and not a `settles_at`.
        //
        // `failed` rows are excluded entirely: the money never left, `balance_for_host`
        // does not count them either, and a row saying a withdrawal happened would be a
        // lie.
        //
        // ponytail: a failed withdrawal vanishes silently. If a host ever needs telling,
        // the fix is a `failed` kind in `projections::wallet` and a row style for it —
        // this predicate loses the `<> 'failed'` and `kind` grows a `case_when`.
        let payouts = payout::table
            .filter(
                payout::host_id
                    .eq(caller_id)
                    .and(payout::status.ne("failed"))
                    .and(payout::created_at.ge(start))
                    .and(payout::created_at.lt(end)),
            )
            .select((
                payout::id.cast::<Text>().concat(":payout"),
                kind::PAYOUT.into_sql::<Text>(),
                payout::amount * -1,
                payout::created_at,
                no_instant(),
                payout::status.eq("requested"),
                // A payout is about no spot and no booking, so all three are null — and
                // each type has to match the other branches' to the letter, or the union
                // does not build. That check is the point of doing this in the DSL.
                None::<String>.into_sql::<Nullable<Text>>(),
                None::<serde_json::Value>.into_sql::<Nullable<Jsonb>>(),
                None::<String>.into_sql::<Nullable<Text>>(),
            ));

        let mut rows: Vec<WalletTransactionProjection> = money_in
            .union_all(money_out)
            .union_all(refund_out)
            .union_all(refund_in)
            .union_all(payouts)
            .load(conn)
            .await?;

        rows.sort_by(|a, b| b.occurred_at.cmp(&a.occurred_at));
        Ok(rows)
    }

    /// The newest instant before `start` that the wallet would show, or `None` at the end
    /// of the history.
    ///
    /// This is the cursor. Without it the client would have to guess: ask for the previous
    /// month, get nothing, and either stop — hiding everything behind a quiet month — or
    /// keep walking backwards forever through a history that ended in 2024.
    ///
    /// **It must select the same rows [`find_month_for_account`] does, and it used not
    /// to.** The three payment branches here now carry the same `JOIN booking` that one
    /// has. Without it a payment whose booking has not been projected yet counted towards
    /// the cursor and was then excluded from the page, so `next_month` could name a month
    /// that came back empty — which is the one failure this function exists to prevent.
    /// The projector applies PAYMENTS and BOOKINGS in separate lanes, so that gap is
    /// ordinary rather than hypothetical.
    ///
    /// Refunds are one branch here where the page has two: both sides read `refunded_at`,
    /// and the newest of `(host OR renter)` is the newest of either taken separately.
    /// `refunded_at < $2` implies `IS NOT NULL`, so that predicate is not repeated.
    ///
    /// Returns the **instant**, not a label. `to_char(…, 'YYYY-MM')` used to format it
    /// here, which was a second spelling of `policy::wallet::label` — the function that
    /// already names the month for the page the caller is looking at. One of them would
    /// eventually have disagreed; now the route labels both with the same code.
    ///
    /// Four `max()`es over four indexed predicates rather than the union above run a
    /// second time. `None` — nothing older exists — is the end of the list.
    ///
    /// **Fully DSL, and the outer `max()` is the reason it can be.** The statement used to
    /// be `SELECT max(t) FROM (<four branches>)`, and diesel has no `FROM (subquery)` — so
    /// it was a string, with a `QueryableByName` struct to name the one column it
    /// returned. But an outer `max()` over four single-row branches is just the largest of
    /// four `Option`s, which is `.flatten().max()` in Rust. Dropping it leaves a plain
    /// `union_all`, which diesel builds, and four rows to fold.
    ///
    /// Still one round trip: the branches are combined in SQL, not issued separately.
    /// The fold happens on four values that have already arrived.
    ///
    /// [`find_month_for_account`]: WalletRepository::find_month_for_account
    pub async fn find_newest_before(
        conn: &mut AsyncPgConnection,
        caller_id: Uuid,
        start: DateTime<Utc>,
    ) -> MyResult<Option<DateTime<Utc>>> {
        let settled = payment::table
            .inner_join(booking::table.on(booking::id.eq(payment::booking_id)))
            .filter(
                payment::status
                    .eq_any(["succeeded", "refunded"])
                    .and(payment::created_at.lt(start)),
            );

        let as_host = settled
            .clone()
            .filter(payment::host_id.eq(caller_id))
            .select(max(payment::created_at));

        let as_renter = settled
            .filter(payment::renter_id.eq(caller_id))
            .select(max(payment::created_at));

        // One branch where the page has two: both sides of a refund read `refunded_at`,
        // and the newest of `(host OR renter)` is the newest of either taken separately.
        // `refunded_at < start` implies `IS NOT NULL`, so that is not spelled out.
        let refunds = payment::table
            .inner_join(booking::table.on(booking::id.eq(payment::booking_id)))
            .filter(
                payment::host_id
                    .eq(caller_id)
                    .or(payment::renter_id.eq(caller_id))
                    .and(payment::refunded_at.lt(start)),
            )
            .select(max(payment::refunded_at));

        // `status <> 'failed'` matches the page, or the cursor names a month whose only
        // row is a failed withdrawal and the client asks for a page that renders empty.
        // No booking join: a payout is about no booking.
        let payouts = payout::table
            .filter(
                payout::host_id
                    .eq(caller_id)
                    .and(payout::status.ne("failed"))
                    .and(payout::created_at.lt(start)),
            )
            .select(max(payout::created_at));

        let newest: Vec<Option<DateTime<Utc>>> = as_host
            .union_all(as_renter)
            .union_all(refunds)
            .union_all(payouts)
            .load(conn)
            .await?;

        Ok(newest.into_iter().flatten().max())
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
    /// One connection and not a pool, so the two statements below cannot come back from
    /// two different pooled connections — the same reason
    /// `PaymentService::earnings_with` takes one.
    ///
    /// **Fully DSL.** It was `sql_query` on the grounds that neither `FILTER` nor the
    /// `::bigint` cast had a spelling; the first was wrong (diesel 2.3 has
    /// `.aggregate_filter()`) and the second needs one declared function, not a string.
    pub async fn balance_for_host(
        conn: &mut AsyncPgConnection,
        host_id: Uuid,
        settled_before: DateTime<Utc>,
    ) -> MyResult<BalanceProjection> {
        // `sum()` over no rows is NULL, and a host who has earned nothing is the
        // ordinary case — so it comes back as `Option<i64>` and Rust's `unwrap_or(0)` is
        // the COALESCE. Expressing "there were no rows" in the type is better than
        // flattening it to a zero inside the database and losing the distinction.
        //
        // `to_bigint` because `sum(bigint)` is **numeric** in Postgres, which does not
        // decode into an `i64` — see the note on that function.
        //
        // `.aggregate_filter()` is `FILTER (WHERE …)`, also native in 2.3. One pass over
        // one index answers both halves, and the two cannot disagree about where the
        // cutoff is — which is the whole reason this is one statement and not two.
        let (earned, pending): (Option<i64>, Option<i64>) = payment::table
            .inner_join(booking::table.on(booking::id.eq(payment::booking_id)))
            .filter(
                payment::host_id
                    .eq(host_id)
                    .and(payment::status.eq("succeeded"))
                    .and(booking::status.eq("confirmed")),
            )
            .select((
                to_bigint(
                    sum(payment::amount).aggregate_filter(booking::ends_at.lt(settled_before)),
                ),
                to_bigint(
                    sum(payment::amount).aggregate_filter(booking::ends_at.ge(settled_before)),
                ),
            ))
            .first(&mut *conn)
            .await?;
        let (earned, pending) = (earned.unwrap_or(0), pending.unwrap_or(0));

        // `status <> 'failed'` — the same set as `PayoutRepository::total_for` in
        // payment-service, which is the authority. A withdrawal in flight still counts
        // (the host cannot have that money again); a refused one does not, and that is
        // the whole of how the money comes back. These two sums are the same arithmetic
        // over two databases: change one and change the other, or the figure beside the
        // withdraw button stops matching what the button will pay.
        let paid_out: Option<i64> = payout::table
            .filter(
                payout::host_id
                    .eq(host_id)
                    .and(payout::status.ne("failed")),
            )
            .select(to_bigint(sum(payout::amount)))
            .first(&mut *conn)
            .await?;
        let paid_out = paid_out.unwrap_or(0);

        Ok(BalanceProjection {
            available_cents: earned - paid_out,
            earned_cents: earned,
            paid_out_cents: paid_out,
            pending_cents: pending,
        })
    }
}
