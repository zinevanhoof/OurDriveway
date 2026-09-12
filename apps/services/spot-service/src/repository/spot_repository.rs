use diesel::prelude::*;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use shared::domain_models::spot::{Spot, SpotPatch};
use shared::error::myerror::MyResult;
use shared::schema::spot::spot;
use uuid::Uuid;

/// The `spot` table.
///
/// Four statements, which is all this service issues. There is no UNIQUE column
/// here, so reads address a row by primary key — which is all anything wanted: the
/// write path resolves one spot before publishing, and the card RPC resolves one spot
/// to label it.
///
/// Stateless, like every repository here — see the note in this module's `mod.rs`.
pub struct SpotRepository;

impl SpotRepository {
    /// `SELECT *`, with nothing to unwrap or default.
    ///
    /// This used to be `record::id(id) AS id` plus `version ?? 0`, `images ?? []` and
    /// `deleted ?? false`, because the id was a record key rather than a column and
    /// older rows held NONE where those fields had been added since. Every one of
    /// them is `NOT NULL DEFAULT …` now.
    ///
    /// `host_id` needed no unwrapping even then — it is a plain uuid column, not a
    /// link, because the user it names lives in another service's database.
    pub async fn find_by_id(conn: &mut AsyncPgConnection, spot_id: Uuid) -> MyResult<Option<Spot>> {
        Ok(spot::table
            .find(spot_id)
            .select(Spot::as_select())
            .first(conn)
            .await
            .optional()?)
    }

    // No `find_for_update` here, deliberately. This service's writes get their row
    // lock from `db::next_version`, which takes `FOR UPDATE` on the row it is about to
    // version. The `FOR UPDATE` that had to be written by hand is in
    // booking-service's *mirror* of this table, where reserve reads a spot it is not
    // versioning — see `SpotMirrorRepository`.

    /// Every spot, deleted ones included, for `SpotService::backfill`.
    ///
    /// Deleted ones matter: their rows stay selectable so a renter's past bookings
    /// keep resolving a title, so a rebuild that skipped them would leave exactly
    /// those bookings unlabelled.
    ///
    /// ponytail: reads the whole table into memory in one pass. Fine for a
    /// maintenance endpoint; page on `id` — `WHERE id > $after ORDER BY id LIMIT $n`
    /// — if listings ever outgrow it.
    pub async fn all(conn: &mut AsyncPgConnection) -> MyResult<Vec<Spot>> {
        Ok(spot::table.select(Spot::as_select()).load(conn).await?)
    }

    /// Insert-or-replace the whole row, keyed by its own id.
    ///
    /// Idempotent by construction, which is what lets the projector replay the same
    /// event.
    ///
    /// `Insertable` binds the struct whole and `AsChangeset` writes the same fields
    /// under `DO UPDATE`, so this needs no edit when [`Spot`] grows a field — what
    /// `UPSERT … CONTENT $row` did. Every column of this table is SPOTS-owned, so
    /// there is nothing a whole-row write can erase.
    pub async fn upsert(conn: &mut AsyncPgConnection, spot_row: Spot) -> MyResult<()> {
        diesel::insert_into(spot::table)
            .values(spot_row.clone())
            .on_conflict(spot::id)
            .do_update()
            .set(spot_row)
            .execute(conn)
            .await?;
        Ok(())
    }

    /// Update only the columns the patch carries. Does not create the row.
    ///
    /// `COALESCE($n, column)` is absent-is-unchanged, and is also the ceiling: no
    /// patch can set a column back to NULL.
    ///
    /// `host_id`, `lng`/`lat`, `address` and `timezone` are absent on purpose: a
    /// spot cannot change hands or move.
    ///
    /// **The binds are positional**, so their order must match the `$n` above — see
    /// the same warning on `UserRepository::patch`.
    pub async fn patch(
        conn: &mut AsyncPgConnection,
        spot_id: Uuid,
        patch: SpotPatch,
    ) -> MyResult<()> {
        diesel::update(spot::table.find(spot_id))
            .set(&patch)
            .execute(conn)
            .await?;
        Ok(())
    }
}
