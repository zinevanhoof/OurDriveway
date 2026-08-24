use std::sync::Arc;

use shared::db::Querier;
use shared::domain_models::spot::{Spot, SpotPatch};
use shared::error::myerror::MyResult;
use surrealdb::{Surreal, engine::remote::ws::Client};
use uuid::Uuid;

/// The `spot` table.
///
/// Three statements, which is all this service issues. There is no UNIQUE column
/// here, so reads address a row by id — which is all anything wanted: the write
/// path resolves one spot before publishing, and the card RPC resolves one spot to
/// label it.
///
/// Defaults to the shared `Arc` connection, so every long-lived repository in the
/// process is one refcount bump rather than one session and one root sign-in
/// each. The projector instead builds `SpotRepository<&Transaction<Client>>` per
/// event, over the transaction it is already inside.
pub struct SpotRepository<Q: Querier = Arc<Surreal<Client>>> {
    pub q: Q,
}

impl<Q: Querier> SpotRepository<Q> {
    /// `*` takes every column, so a new field on [`Spot`] needs no edit here.
    /// Three are spelled out because `*` returns them in a shape the struct cannot
    /// deserialize: `id` is the record key `spot:⟨uuid⟩` where the struct holds a
    /// plain uuid, and `images`/`deleted` are NONE on rows written before those
    /// columns existed.
    ///
    /// `owner_id` is deliberately *not* unwrapped — it is a plain uuid column, not
    /// a link, so `record::id(owner_id)` would run against a value that is not a
    /// record id.
    ///
    /// An explicit alias beats `*` for the same name in either order — checked
    /// against SurrealDB 3.2.4 rather than assumed.
    pub async fn find_by_id(&self, spot_id: Uuid) -> MyResult<Option<Spot>> {
        Ok(self
            .q
            .q("SELECT record::id(id) AS id,
                       images  ?? []    AS images,
                       deleted ?? false AS deleted,
                       *
                FROM ONLY type::record('spot', $v)")
            .bind(("v", spot_id))
            .await?
            .take(0)?)
    }

    /// Insert-or-replace the whole row, keyed by its own id.
    ///
    /// Idempotent by construction, which is what lets the projector replay the same
    /// event. `CONTENT $row` binds the struct whole, so adding a field to [`Spot`]
    /// needs no change here — and every column of this table is SPOTS-owned, so
    /// there is nothing a whole-row write can erase.
    ///
    /// The row carries its own `id` and the statement also names one. SurrealDB
    /// requires them to agree and errors if they do not, which makes this a free
    /// assertion rather than a risk.
    pub async fn upsert(&self, spot: Spot) -> MyResult<()> {
        let id = spot.id;
        self.q
            .q("UPSERT type::record('spot', $id) CONTENT $row")
            .bind(("id", id))
            .bind(("row", spot))
            .await?
            .check()?;
        Ok(())
    }

    /// Update only the columns the patch carries. Does not create the row.
    ///
    /// `?? column` means absent-is-unchanged, and is also the ceiling: no patch can
    /// set a column back to NONE. The eight columns here are every column
    /// [`SpotPatch`] carries — add one there and it has to be added here too.
    ///
    /// `owner_id`, `shard`, `location`, `address` and `timezone` are absent on
    /// purpose: a spot cannot change hands or move.
    pub async fn patch(&self, spot_id: Uuid, patch: SpotPatch) -> MyResult<()> {
        patch
            .bind(
                self.q
                    .q("UPDATE type::record('spot', $v) SET
                            title          = $title          ?? title,
                            description    = $description    ?? description,
                            price_per_hour = $price_per_hour ?? price_per_hour,
                            images         = $images         ?? images,
                            availability   = $availability   ?? availability,
                            active         = $active         ?? active,
                            deleted        = $deleted        ?? deleted,
                            updated_at     = $updated_at     ?? updated_at;")
                    .bind(("v", spot_id)),
            )
            .await?
            .check()?;
        Ok(())
    }
}
