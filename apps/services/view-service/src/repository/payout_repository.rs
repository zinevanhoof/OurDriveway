use diesel::prelude::*;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use shared::domain_models::view::payout::ViewPayout;
use shared::error::myerror::MyResult;
use shared::schema::view::payout;

/// The `payout` table in the read model — withdrawal history, nothing else.
///
/// A write statement and no read: `find_all_by_host_id` served
/// `GET /api/view/me/payouts`, which the wallet replaced. Withdrawals are still read,
/// but as one of the four sources in `WalletRepository::find_month` — a separate
/// history list beside a wallet that already contains it is a second place for the same
/// rows to be wrong.
///
/// The rule that read carried — `host_id = $1`, the whole query rather than a clause,
/// because a payout is visible to exactly one person — is unchanged in the wallet's
/// payout branch.
pub struct ViewPayoutRepository;

impl ViewPayoutRepository {
    /// Insert-or-replace the whole row.
    ///
    /// One statement, where this used to be two. A `CONTENT $row` write cleared the
    /// `host` record link, so `link_host` had to put it back in the same transaction
    /// — two halves that both had to run, with nothing in Rust connecting them, and a
    /// live test existing solely to prove they did. `host_id` is a plain uuid column
    /// written by this statement like any other.
    pub async fn upsert(conn: &mut AsyncPgConnection, row: ViewPayout) -> MyResult<()> {
        diesel::insert_into(payout::table)
            .values(row.clone())
            .on_conflict(payout::id)
            .do_update()
            .set(row)
            .execute(conn)
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
        conn: &mut AsyncPgConnection,
        id: uuid::Uuid,
        status: &str,
    ) -> MyResult<()> {
        diesel::update(payout::table.find(id))
            .set(payout::status.eq(status))
            .execute(conn)
            .await?;
        Ok(())
    }
}
