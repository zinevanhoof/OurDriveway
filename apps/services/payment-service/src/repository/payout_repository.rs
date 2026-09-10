use diesel::dsl::sum;
use diesel::prelude::*;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use shared::diesel_ext::to_bigint;
use shared::domain_models::payment::payout::status;
use shared::domain_models::payment::{Payout, PayoutPatch};
use shared::error::myerror::MyResult;
use shared::schema::payment::payout;
use uuid::Uuid;

/// The `payout` table.
pub struct PayoutRepository;

impl PayoutRepository {
    pub async fn upsert(conn: &mut AsyncPgConnection, row: Payout) -> MyResult<()> {
        diesel::insert_into(payout::table)
            .values(row.clone())
            .on_conflict(payout::id)
            .do_update()
            .set(row)
            .execute(conn)
            .await?;
        Ok(())
    }

    /// One payout, by id. The worker's only read: it is handed an id on an event and
    /// needs the amount, the host and — above all — the status.
    pub async fn find_by_id(conn: &mut AsyncPgConnection, id: Uuid) -> MyResult<Option<Payout>> {
        Ok(payout::table
            .find(id)
            .select(Payout::as_select())
            .first(conn)
            .await
            .optional()?)
    }

    /// Patch a payout only if it is currently in one of `from` — the same shape, and
    /// the same reasoning, as `PaymentRepository::transition`.
    ///
    /// The guard is what makes a redelivered `PayoutRequested` harmless: the worker's
    /// second run finds `paid` rather than `requested`, and this writes nothing. It is
    /// the cheap half of the defence; the idempotency key on the Stripe call is the
    /// half that holds when the crash happened before this ever ran.
    ///
    /// **The binds are positional.** Both patchable text columns are
    /// `Option<String>`, so a swapped pair compiles and writes a transfer id into
    /// `failure_reason`.
    pub async fn transition(
        conn: &mut AsyncPgConnection,
        payout_id: Uuid,
        from: &[&str],
        patch: PayoutPatch,
    ) -> MyResult<()> {
        diesel::update(
            payout::table.find(payout_id).filter(
                payout::status.eq_any(from.iter().map(|s| s.to_string()).collect::<Vec<_>>()),
            ),
        )
        .set(&patch)
        .execute(conn)
        .await?;
        Ok(())
    }

    /// Every payout, for `PaymentService::backfill`.
    ///
    /// Payouts and not payments: view-service projects `PayoutRequested` and
    /// deliberately nothing else off PAYMENTS — what a renter was charged is this
    /// service's to answer — so the payment table has no downstream projection to
    /// rebuild.
    ///
    /// ponytail: whole table in one pass, same ceiling and same fix as the others.
    pub async fn all(conn: &mut AsyncPgConnection) -> MyResult<Vec<Payout>> {
        Ok(payout::table.select(Payout::as_select()).load(conn).await?)
    }

    /// Everything this host has already withdrawn **or is withdrawing**.
    ///
    /// A sum, so it never fetches the rows to add up one column in Rust.
    ///
    /// `COALESCE` because SUM over no rows is NULL, and a host who has never withdrawn
    /// is the ordinary case. `::bigint` because `SUM(bigint)` returns **numeric** —
    /// Postgres widens to avoid overflow, and nothing decodes numeric into an i64.
    ///
    /// # The status filter is the money rule
    ///
    /// `requested` counts. A withdrawal whose transfer is still in flight is money the
    /// host cannot have again, or a fast second withdrawal takes it twice — the
    /// advisory lock serialises the two requests but the second one's balance has to
    /// *see* the first.
    ///
    /// `failed` does not count, and that is the entire refund mechanism. There is no
    /// stored balance to credit back: available is derived on every read, so a row that
    /// stops matching hands the money back for free — exactly as a refunded booking
    /// drops out of `PaymentRepository::earned`.
    ///
    /// view-service's wallet queries filter on the same three values. They are the same
    /// arithmetic over two databases and must not drift.
    pub async fn total_for(conn: &mut AsyncPgConnection, host_id: &Uuid) -> MyResult<i64> {
        // `sum()` over no rows is NULL — a host who has never withdrawn is the ordinary
        // case — so it arrives as `None` and `unwrap_or(0)` is the default. Deliberately
        // not `COALESCE(…, 0)` in SQL, which would flatten "never withdrew" and "withdrew
        // nothing" into one value before Rust could tell them apart.
        //
        // `to_bigint` because `sum(bigint)` is numeric in Postgres, which does not decode
        // into an `i64` — see the note on that function in `shared::diesel_ext`.
        let total: Option<i64> = payout::table
            .filter(
                payout::host_id
                    .eq(host_id)
                    .and(payout::status.eq_any(status::COUNTED)),
            )
            .select(to_bigint(sum(payout::amount_cents)))
            .first(conn)
            .await?;

        Ok(total.unwrap_or(0))
    }

    /// Takes the per-host lock that serialises two concurrent withdrawals.
    ///
    /// # Why an advisory lock and not a row
    ///
    /// A balance is derived (`earnings − Σ payouts`), never stored, so two
    /// double-clicked withdrawals read the same available amount and insert two
    /// *different* payout rows — different keys, nothing collides, money out twice.
    ///
    /// Locking the existing payout rows does not help: the row that changes the answer
    /// is one that **does not exist yet**, and a first-time withdrawer has none to
    /// lock. That phantom is what Read Committed permits, and it is why a `host` table
    /// used to exist holding nothing but a version to bump — a row invented purely to
    /// be contended on. Under Read Committed that bump would not conflict either, so
    /// the table was deleted rather than ported.
    ///
    /// `pg_advisory_xact_lock` locks the host id itself. Xact-scoped, never the
    /// session variant: it is released by COMMIT or ROLLBACK, so there is no unlock to
    /// forget on the `?` early-returns this path is full of, and no `Drop` that would
    /// have to await one.
    ///
    /// **The balance query must come after this**, inside the same transaction. That
    /// is the half that is easy to miss: a snapshot taken before the lock is stale
    /// however long the lock is then held.
    pub async fn lock_host(conn: &mut AsyncPgConnection, host_id: &Uuid) -> MyResult<()> {
        // Stays `sql_query`: `pg_advisory_xact_lock` is a void-returning function call
        // with no DSL spelling, and wrapping it would hide the one statement whose
        // ORDER relative to the balance read is the entire mechanism.
        diesel::sql_query("SELECT pg_advisory_xact_lock($1)")
            .bind::<diesel::sql_types::BigInt, _>(shared::db::advisory_key(host_id))
            .execute(conn)
            .await?;
        Ok(())
    }
}
