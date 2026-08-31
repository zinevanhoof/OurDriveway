use shared::domain_models::view::payout::ViewPayout;
use shared::error::myerror::MyResult;
use shared::projections::payout::PayoutListItem;
use sqlx::PgExecutor;
use uuid::Uuid;

/// The `payout` table in the read model — withdrawal history, nothing else.
pub struct ViewPayoutRepository;

impl ViewPayoutRepository {
    /// The caller's own withdrawals, newest first.
    ///
    /// **The `payout` table's select rule**, which was
    /// `FOR select WHERE owner_id = record::id($auth)` in view-schema.surql. Unlike
    /// `spot` and `booking` it has no public half at all: a payout is visible to
    /// exactly one person, so the predicate is the whole query rather than a clause
    /// appended to it.
    ///
    /// Served by `payout_owner (owner_id, created_at)`.
    pub async fn find_all_by_owner_id(
        ex: impl PgExecutor<'_>,
        owner_id: Uuid,
    ) -> MyResult<Vec<PayoutListItem>> {
        Ok(sqlx::query_as(
            "SELECT id, amount, created_at FROM payout
              WHERE owner_id = $1
              ORDER BY created_at DESC",
        )
        .bind(owner_id)
        .fetch_all(ex)
        .await?)
    }

    /// Insert-or-replace the whole row.
    ///
    /// One statement, where this used to be two. A `CONTENT $row` write cleared the
    /// `owner` record link, so `link_owner` had to put it back in the same transaction
    /// — two halves that both had to run, with nothing in Rust connecting them, and a
    /// live test existing solely to prove they did. `owner_id` is a plain uuid column
    /// written by this statement like any other.
    pub async fn upsert(ex: impl PgExecutor<'_>, payout: ViewPayout) -> MyResult<()> {
        sqlx::query(
            "INSERT INTO payout (id, version, owner_id, amount, created_at)
                  VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (id) DO UPDATE SET
                 version    = EXCLUDED.version,
                 owner_id   = EXCLUDED.owner_id,
                 amount     = EXCLUDED.amount,
                 created_at = EXCLUDED.created_at",
        )
        .bind(payout.id)
        .bind(payout.version as i64)
        .bind(payout.owner_id)
        .bind(payout.amount)
        .bind(payout.created_at)
        .execute(ex)
        .await?;
        Ok(())
    }
}
