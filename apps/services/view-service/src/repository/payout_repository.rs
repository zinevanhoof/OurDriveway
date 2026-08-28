use std::sync::Arc;

use shared::db::Querier;
use shared::domain_models::view::payout::ViewPayout;
use shared::error::myerror::MyResult;
use surrealdb::{Surreal, engine::remote::ws::Client};
use uuid::Uuid;

/// The `payout` table in the read model — withdrawal history, nothing else.
pub struct ViewPayoutRepository<Q: Querier = Arc<Surreal<Client>>> {
    pub q: Q,
}

impl<Q: Querier> ViewPayoutRepository<Q> {
    /// Insert-or-replace the whole row.
    ///
    /// `CONTENT` clears the `owner` link, which [`ViewPayoutRepository::link_owner`]
    /// puts back in the same transaction. That is deliberate rather than a hazard:
    /// the link resolves to NONE when the host has not been projected yet, so it
    /// cannot be written from the row shape.
    pub async fn upsert(&self, payout: ViewPayout) -> MyResult<()> {
        let id = payout.id;
        self.q
            .q("UPSERT type::record('payout', $id) CONTENT $row")
            .bind(("id", id))
            .bind(("row", payout))
            .await?
            .check()?;
        Ok(())
    }

    /// Points `owner` at the host's row.
    ///
    /// Written unconditionally rather than resolved through a subquery — see
    /// `ViewBookingRepository::link_refs` for why that subquery was the bug and not
    /// the safety.
    ///
    /// Runs straight after the `upsert` that cleared it, in the same transaction, so
    /// there is no existing link to preserve and no `= NONE` scope needed.
    pub async fn link_owner(&self, payout_id: &Uuid, owner_id: &Uuid) -> MyResult<()> {
        self.q
            .q("UPDATE type::record('payout', $id)
                SET owner = type::record('user', $owner);")
            .bind(("id", *payout_id))
            .bind(("owner", *owner_id))
            .await?
            .check()?;
        Ok(())
    }
}
