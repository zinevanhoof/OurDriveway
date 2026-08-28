use std::sync::Arc;

use shared::db::Querier;
use shared::domain_models::booking::{SpotMirror, SpotMirrorPatch};
use shared::error::myerror::MyResult;
use surrealdb::types::vars;
use surrealdb::{Surreal, engine::remote::ws::Client};
use uuid::Uuid;

/// booking-service's local mirror of the `spot` table.
///
/// Every SPOTS write goes through [`SpotMirrorRepository::merge`], never a
/// whole-row `CONTENT` — see [`SpotMirror`] for why that would erase this
/// table's one BOOKINGS-derived column.
pub struct SpotMirrorRepository<Q: Querier = Arc<Surreal<Client>>> {
    pub q: Q,
}

impl<Q: Querier> SpotMirrorRepository<Q> {
    /// `*` takes every column, so a new field on [`SpotMirror`] needs no edit
    /// here. Three are spelled out because `*` returns them in a shape the struct
    /// cannot deserialize.
    ///
    /// `id` is the record key `spot:⟨uuid⟩` where the struct holds a plain uuid.
    /// The other two are the columns a partially-built row will not have — and on
    /// *this* table that is the normal cold-rebuild case, not an edge one, because
    /// the two projectors advance independently and either side can create the row.
    /// Without the defaults they come back NONE and the whole read fails.
    ///
    /// An explicit alias beats `*` for the same name in either order — checked
    /// against SurrealDB 3.2.4 rather than assumed.
    pub async fn find_by_id(&self, spot_id: Uuid) -> MyResult<Option<SpotMirror>> {
        Ok(self
            .q
            .q("SELECT record::id(id) AS id,
                       deleted      ?? false AS deleted,
                       bookings_seq ?? 0     AS bookings_seq,
                       *
                FROM ONLY type::record('spot', $v)")
            .bind(("v", spot_id))
            .await?
            .take(0)?)
    }

    /// Apply a SPOTS event to the mirror, creating the row if it is not there yet.
    ///
    /// `UPSERT`, not `UPDATE`: the two projectors advance independently, so the
    /// event that would have created this row is routinely *not* the first one to
    /// arrive. And a `SET` list, not `CONTENT`: the columns named here are every
    /// column [`SpotMirrorPatch`] carries, which is every SPOTS-owned column —
    /// `bookings_seq` is absent by construction, so this statement cannot touch it
    /// no matter what a caller passes.
    ///
    /// `?? column` means absent-is-unchanged. On a row being created that resolves
    /// to NONE, so the table's own DEFAULTs decide the rest.
    pub async fn merge(&self, spot_id: Uuid, patch: SpotMirrorPatch) -> MyResult<()> {
        patch
            .bind(
                self.q
                    .q("UPSERT type::record('spot', $v) SET
                            owner_id       = $owner_id       ?? owner_id,
                            price_per_hour = $price_per_hour ?? price_per_hour,
                            availability   = $availability   ?? availability,
                            timezone       = $timezone       ?? timezone,
                            active         = $active         ?? active,
                            deleted        = $deleted        ?? deleted;")
                    .bind(("v", spot_id)),
            )
            .await?
            .check()?;
        Ok(())
    }

    /// Advances this spot's compare-and-swap cursor to `seq`.
    ///
    /// Called for every BOOKINGS event on the spot, from inside the transaction that
    /// wrote the booking row — so the cursor can never run ahead of the rows reserve
    /// reads, which is the invariant that used to be bought by writing the cursor
    /// and the slot map in one statement.
    ///
    /// `math::max` is load-bearing rather than decorative: a redelivered older
    /// message must not rewind `bookings_seq`, or every subsequent reserve would
    /// assert a sequence below the subject's head and be refused forever. A bound
    /// patch can only assign, which is why this is its own statement rather than a
    /// column on [`SpotMirrorPatch`].
    pub async fn advance(&self, spot_id: &Uuid, seq: u64) -> MyResult<()> {
        self.q
            .q("UPSERT type::record('spot', $id) SET
                    bookings_seq = math::max([bookings_seq ?? 0, $seq]);")
            .bind(vars! {
                id:  *spot_id,
                seq: seq as i64,
            })
            .await?
            .check()?;
        Ok(())
    }
}
