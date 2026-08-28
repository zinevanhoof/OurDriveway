use std::sync::Arc;

use shared::db::Querier;
use shared::domain_models::payment::Payout;
use shared::error::myerror::MyResult;
use surrealdb::{Surreal, engine::remote::ws::Client};
use uuid::Uuid;

/// The `payout` table.
pub struct PayoutRepository<Q: Querier = Arc<Surreal<Client>>> {
    pub q: Q,
}

impl<Q: Querier> PayoutRepository<Q> {
    pub async fn upsert(&self, payout: Payout) -> MyResult<()> {
        let id = payout.id;
        self.q
            .q("UPSERT type::record('payout', $id) CONTENT $row")
            .bind(("id", id))
            .bind(("row", payout))
            .await?
            .check()?;
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
    pub async fn all(&self) -> MyResult<Vec<Payout>> {
        Ok(self
            .q
            .q("SELECT record::id(id) AS id, version ?? 0 AS version, * FROM payout")
            .await?
            .take(0)?)
    }

    /// Everything this host has already withdrawn.
    ///
    /// A sum, so it never fetches the rows to add up one column in Rust.
    pub async fn total_for(&self, owner_id: &Uuid) -> MyResult<i64> {
        let sum: Option<i64> = self
            .q
            .q("SELECT VALUE math::sum(amount_cents) FROM ONLY (
                    SELECT amount_cents FROM payout WHERE owner_id = $o
                ) GROUP ALL")
            .bind(("o", *owner_id))
            .await?
            .take(0)?;
        Ok(sum.unwrap_or(0))
    }
}
