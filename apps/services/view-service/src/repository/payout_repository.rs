use shared::domain_models::view::payout::ViewPayout;
use shared::error::myerror::MyResult;
use sqlx::PgExecutor;

/// The `payout` table in the read model — withdrawal history, nothing else.
///
/// A write statement and no read: `find_all_by_owner_id` served
/// `GET /api/view/me/payouts`, which the wallet replaced. Withdrawals are still read,
/// but as one of the four sources in `WalletRepository::find_month` — a separate
/// history list beside a wallet that already contains it is a second place for the same
/// rows to be wrong.
///
/// The rule that read carried — `owner_id = $1`, the whole query rather than a clause,
/// because a payout is visible to exactly one person — is unchanged in the wallet's
/// payout branch.
pub struct ViewPayoutRepository;

impl ViewPayoutRepository {
    /// Insert-or-replace the whole row.
    ///
    /// One statement, where this used to be two. A `CONTENT $row` write cleared the
    /// `owner` record link, so `link_owner` had to put it back in the same transaction
    /// — two halves that both had to run, with nothing in Rust connecting them, and a
    /// live test existing solely to prove they did. `owner_id` is a plain uuid column
    /// written by this statement like any other.
    pub async fn upsert(ex: impl PgExecutor<'_>, payout: ViewPayout) -> MyResult<()> {
        sqlx::query(
            "INSERT INTO payout (id, version, owner_id, amount, status, created_at)
                  VALUES ($1, $2, $3, $4, $5, $6)
             ON CONFLICT (id) DO UPDATE SET
                 version    = EXCLUDED.version,
                 owner_id   = EXCLUDED.owner_id,
                 amount     = EXCLUDED.amount,
                 status     = EXCLUDED.status,
                 created_at = EXCLUDED.created_at",
        )
        .bind(payout.id)
        .bind(payout.version as i64)
        .bind(payout.owner_id)
        .bind(payout.amount)
        .bind(payout.status)
        .bind(payout.created_at)
        .execute(ex)
        .await?;
        Ok(())
    }

    /// Where the withdrawal got to, once the worker knows.
    ///
    /// No patch struct and no `from` guard, unlike `ViewPaymentRepository::transition`:
    /// there is one column, and the ordering that would need guarding is already
    /// guaranteed. A payout's three events share one subject, so they share one
    /// projector lane and arrive in the order payment-service published them.
    ///
    /// `WHERE id = $1` and nothing else, deliberately. A row that is not here yet
    /// cannot happen for the same reason — but if it ever did, this writing nothing is
    /// the correct outcome: the request's own event creates the row, and it is ahead of
    /// this one in the same lane.
    pub async fn set_status(
        ex: impl PgExecutor<'_>,
        id: uuid::Uuid,
        status: &str,
    ) -> MyResult<()> {
        sqlx::query("UPDATE payout SET status = $2 WHERE id = $1")
            .bind(id)
            .bind(status)
            .execute(ex)
            .await?;
        Ok(())
    }
}
