use shared::domain_models::payment::payout::status;
use shared::domain_models::payment::{Payout, PayoutPatch};
use shared::error::myerror::MyResult;
use sqlx::PgExecutor;
use uuid::Uuid;

/// The `payout` table.
pub struct PayoutRepository;

impl PayoutRepository {
    pub async fn upsert(ex: impl PgExecutor<'_>, payout: Payout) -> MyResult<()> {
        sqlx::query(
            "INSERT INTO payout
                 (id, version, owner_id, amount_cents, status, transfer_id,
                  failure_reason, created_at)
                  VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
             ON CONFLICT (id) DO UPDATE SET
                 version        = EXCLUDED.version,
                 owner_id       = EXCLUDED.owner_id,
                 amount_cents   = EXCLUDED.amount_cents,
                 status         = EXCLUDED.status,
                 transfer_id    = EXCLUDED.transfer_id,
                 failure_reason = EXCLUDED.failure_reason,
                 created_at     = EXCLUDED.created_at",
        )
        .bind(payout.id)
        .bind(payout.version as i64)
        .bind(payout.owner_id)
        .bind(payout.amount_cents)
        .bind(payout.status)
        .bind(payout.transfer_id)
        .bind(payout.failure_reason)
        .bind(payout.created_at)
        .execute(ex)
        .await?;
        Ok(())
    }

    /// One payout, by id. The worker's only read: it is handed an id on an event and
    /// needs the amount, the owner and — above all — the status.
    pub async fn find_by_id(ex: impl PgExecutor<'_>, id: Uuid) -> MyResult<Option<Payout>> {
        Ok(sqlx::query_as("SELECT * FROM payout WHERE id = $1")
            .bind(id)
            .fetch_optional(ex)
            .await?)
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
        ex: impl PgExecutor<'_>,
        payout_id: Uuid,
        from: &[&str],
        patch: PayoutPatch,
    ) -> MyResult<()> {
        sqlx::query(
            "UPDATE payout SET
                 status         = COALESCE($3, status),
                 transfer_id    = COALESCE($4, transfer_id),
                 failure_reason = COALESCE($5, failure_reason)
             WHERE id = $1 AND status = ANY($2)",
        )
        .bind(payout_id)
        .bind(from.iter().map(|s| s.to_string()).collect::<Vec<_>>())
        .bind(patch.status)
        .bind(patch.transfer_id)
        .bind(patch.failure_reason)
        .execute(ex)
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
    pub async fn all(ex: impl PgExecutor<'_>) -> MyResult<Vec<Payout>> {
        Ok(sqlx::query_as("SELECT * FROM payout").fetch_all(ex).await?)
    }

    /// Everything this host has already withdrawn **or is withdrawing**.
    ///
    /// A sum, so it never fetches the rows to add up one column in Rust.
    ///
    /// `COALESCE` because SUM over no rows is NULL, and a host who has never withdrawn
    /// is the ordinary case. `::bigint` because `SUM(bigint)` returns **numeric** —
    /// Postgres widens to avoid overflow, and sqlx will not decode that into an i64.
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
    pub async fn total_for(ex: impl PgExecutor<'_>, owner_id: &Uuid) -> MyResult<i64> {
        Ok(sqlx::query_scalar(
            "SELECT COALESCE(SUM(amount_cents), 0)::bigint
               FROM payout
              WHERE owner_id = $1 AND status = ANY($2)",
        )
        .bind(owner_id)
        .bind(
            status::COUNTED
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>(),
        )
        .fetch_one(ex)
        .await?)
    }

    /// Takes the per-owner lock that serialises two concurrent withdrawals.
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
    /// `pg_advisory_xact_lock` locks the owner id itself. Xact-scoped, never the
    /// session variant: it is released by COMMIT or ROLLBACK, so there is no unlock to
    /// forget on the `?` early-returns this path is full of, and no `Drop` that would
    /// have to await one.
    ///
    /// **The balance query must come after this**, inside the same transaction. That
    /// is the half that is easy to miss: a snapshot taken before the lock is stale
    /// however long the lock is then held.
    pub async fn lock_owner(ex: impl PgExecutor<'_>, owner_id: &Uuid) -> MyResult<()> {
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(shared::db::advisory_key(owner_id))
            .execute(ex)
            .await?;
        Ok(())
    }
}
