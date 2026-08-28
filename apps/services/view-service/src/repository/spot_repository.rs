use std::sync::Arc;

use shared::db::Querier;
use shared::domain_models::view::spot::ViewSpotPatch;
use shared::error::myerror::MyResult;
use surrealdb::{Surreal, engine::remote::ws::Client};
use uuid::Uuid;

/// The `spot` table in the read model.
///
/// Every write from the SPOTS stream is a `SET` list, never a whole-row `CONTENT` —
/// see [`shared::domain_models::view::spot::ViewSpot`] for why that would erase the
/// owner link. The columns named below are exactly the ones [`ViewSpotPatch`]
/// carries, and `owner` is not among them.
pub struct ViewSpotRepository<Q: Querier = Arc<Surreal<Client>>> {
    pub q: Q,
}

impl<Q: Querier> ViewSpotRepository<Q> {
    /// Apply a `SpotCreated`, creating the row if it is not there yet.
    ///
    /// `UPSERT` rather than `CREATE`, so a redelivered `SpotCreated` is idempotent.
    /// Every column of `ViewSpotPatch::created` is `Some`, so this replaces all of
    /// them while leaving `owner` alone.
    pub async fn merge(&self, spot_id: Uuid, patch: ViewSpotPatch) -> MyResult<()> {
        patch
            .bind(self.q.q(Self::SET_LIST_UPSERT).bind(("v", spot_id)))
            .await?
            .check()?;
        Ok(())
    }

    /// Apply an edit. Does not create the row — an edit for a spot that was never
    /// created is a no-op, not a partial row.
    pub async fn patch(&self, spot_id: Uuid, patch: ViewSpotPatch) -> MyResult<()> {
        patch
            .bind(self.q.q(Self::SET_LIST_UPDATE).bind(("v", spot_id)))
            .await?
            .check()?;
        Ok(())
    }

    /// The thirteen SPOTS-owned columns, in `UPSERT` and `UPDATE` form.
    ///
    /// Spelled out twice rather than assembled, so each statement reads as one
    /// piece. They differ only in the verb: whether a missing row is created.
    const SET_LIST_UPSERT: &'static str = "UPSERT type::record('spot', $v) SET
             owner_id       = $owner_id       ?? owner_id,
             title          = $title          ?? title,
             description    = $description    ?? description,
             price_per_hour = $price_per_hour ?? price_per_hour,
             images         = $images         ?? images,
             location       = $location       ?? location,
             active         = $active         ?? active,
             deleted        = $deleted        ?? deleted,
             address        = $address        ?? address,
             availability   = $availability   ?? availability,
             timezone       = $timezone       ?? timezone,
             created_at     = $created_at     ?? created_at,
             updated_at     = $updated_at     ?? updated_at;";

    const SET_LIST_UPDATE: &'static str = "UPDATE type::record('spot', $v) SET
             owner_id       = $owner_id       ?? owner_id,
             title          = $title          ?? title,
             description    = $description    ?? description,
             price_per_hour = $price_per_hour ?? price_per_hour,
             images         = $images         ?? images,
             location       = $location       ?? location,
             active         = $active         ?? active,
             deleted        = $deleted        ?? deleted,
             address        = $address        ?? address,
             availability   = $availability   ?? availability,
             timezone       = $timezone       ?? timezone,
             created_at     = $created_at     ?? created_at,
             updated_at     = $updated_at     ?? updated_at;";

    /// Points `owner` at the host's row.
    ///
    /// Written unconditionally rather than resolved through a subquery — see
    /// `ViewBookingRepository::link_refs` for why that subquery was the bug and not
    /// the safety.
    ///
    /// Still scoped `WHERE owner = NONE`, unlike `link_refs`: that one runs after an
    /// `upsert` that cleared the link, this one after a `merge` that preserved it, so
    /// there *is* an existing value here and no reason to rewrite it.
    pub async fn link_owner(&self, spot_id: &Uuid, owner_id: &Uuid) -> MyResult<()> {
        self.q
            .q("UPDATE type::record('spot', $id)
                SET owner = type::record('user', $owner)
                WHERE owner = NONE;")
            .bind(("id", *spot_id))
            .bind(("owner", *owner_id))
            .await?
            .check()?;
        Ok(())
    }
}
