use shared::domain_models::payment::Payout;
use shared::error::myerror::MyResult;
use sqlx::PgExecutor;
use uuid::Uuid;

/// The `payout` table.
pub struct PayoutRepository;

impl PayoutRepository {
    pub async fn upsert(ex: impl PgExecutor<'_>, payout: Payout) -> MyResult<()> {
        sqlx::query(
            "INSERT INTO payout (id, version, owner_id, amount_cents, created_at)
                  VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (id) DO UPDATE SET
                 version      = EXCLUDED.version,
                 owner_id     = EXCLUDED.owner_id,
                 amount_cents = EXCLUDED.amount_cents,
                 created_at   = EXCLUDED.created_at",
        )
        .bind(payout.id)
        .bind(payout.version as i64)
        .bind(payout.owner_id)
        .bind(payout.amount_cents)
        .bind(payout.created_at)
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

    /// Everything this host has already withdrawn.
    ///
    /// A sum, so it never fetches the rows to add up one column in Rust.
    ///
    /// `COALESCE` because SUM over no rows is NULL, and a host who has never withdrawn
    /// is the ordinary case. `::bigint` because `SUM(bigint)` returns **numeric** —
    /// Postgres widens to avoid overflow, and sqlx will not decode that into an i64.
    pub async fn total_for(ex: impl PgExecutor<'_>, owner_id: &Uuid) -> MyResult<i64> {
        Ok(sqlx::query_scalar(
            "SELECT COALESCE(SUM(amount_cents), 0)::bigint FROM payout WHERE owner_id = $1",
        )
        .bind(owner_id)
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
