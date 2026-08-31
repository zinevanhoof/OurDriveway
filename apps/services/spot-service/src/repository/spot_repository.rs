use shared::domain_models::spot::{Spot, SpotPatch};
use shared::error::myerror::MyResult;
use sqlx::PgExecutor;
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
    /// `owner_id` needed no unwrapping even then — it is a plain uuid column, not a
    /// link, because the user it names lives in another service's database.
    pub async fn find_by_id(ex: impl PgExecutor<'_>, spot_id: Uuid) -> MyResult<Option<Spot>> {
        Ok(sqlx::query_as("SELECT * FROM spot WHERE id = $1")
            .bind(spot_id)
            .fetch_optional(ex)
            .await?)
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
    pub async fn all(ex: impl PgExecutor<'_>) -> MyResult<Vec<Spot>> {
        Ok(sqlx::query_as("SELECT * FROM spot").fetch_all(ex).await?)
    }

    /// Insert-or-replace the whole row, keyed by its own id.
    ///
    /// Idempotent by construction, which is what lets the projector replay the same
    /// event.
    ///
    /// The columns are spelled out because sqlx has no whole-struct write; `UPSERT …
    /// CONTENT $row` bound the struct and needed no edit when [`Spot`] grew a field.
    /// Every column of this table is SPOTS-owned, so there is still nothing a
    /// whole-row write can erase.
    pub async fn upsert(ex: impl PgExecutor<'_>, spot: Spot) -> MyResult<()> {
        sqlx::query(
            "INSERT INTO spot
                 (id, version, owner_id, title, description, price_per_hour, images,
                  lng, lat, active, deleted, address, availability, timezone,
                  created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)
             ON CONFLICT (id) DO UPDATE SET
                 version        = EXCLUDED.version,
                 owner_id       = EXCLUDED.owner_id,
                 title          = EXCLUDED.title,
                 description    = EXCLUDED.description,
                 price_per_hour = EXCLUDED.price_per_hour,
                 images         = EXCLUDED.images,
                 lng            = EXCLUDED.lng,
                 lat            = EXCLUDED.lat,
                 active         = EXCLUDED.active,
                 deleted        = EXCLUDED.deleted,
                 address        = EXCLUDED.address,
                 availability   = EXCLUDED.availability,
                 timezone       = EXCLUDED.timezone,
                 created_at     = EXCLUDED.created_at,
                 updated_at     = EXCLUDED.updated_at",
        )
        .bind(spot.id)
        .bind(spot.version as i64)
        .bind(spot.owner_id)
        .bind(spot.title)
        .bind(spot.description)
        .bind(spot.price_per_hour)
        .bind(spot.images)
        .bind(spot.lng)
        .bind(spot.lat)
        .bind(spot.active)
        .bind(spot.deleted)
        // `Json(…)` is the write side of the model's `#[sqlx(json)]`: the column is
        // jsonb, and these are serialized whole because nothing ever queries into
        // them.
        .bind(sqlx::types::Json(spot.address))
        .bind(sqlx::types::Json(spot.availability))
        .bind(spot.timezone)
        .bind(spot.created_at)
        .bind(spot.updated_at)
        .execute(ex)
        .await?;
        Ok(())
    }

    /// Update only the columns the patch carries. Does not create the row.
    ///
    /// `COALESCE($n, column)` is absent-is-unchanged, and is also the ceiling: no
    /// patch can set a column back to NULL.
    ///
    /// `owner_id`, `lng`/`lat`, `address` and `timezone` are absent on purpose: a
    /// spot cannot change hands or move.
    ///
    /// **The binds are positional**, so their order must match the `$n` above — see
    /// the same warning on `UserRepository::patch`.
    pub async fn patch(ex: impl PgExecutor<'_>, spot_id: Uuid, patch: SpotPatch) -> MyResult<()> {
        sqlx::query(
            "UPDATE spot SET
                 title          = COALESCE($2, title),
                 description    = COALESCE($3, description),
                 price_per_hour = COALESCE($4, price_per_hour),
                 images         = COALESCE($5, images),
                 availability   = COALESCE($6, availability),
                 active         = COALESCE($7, active),
                 deleted        = COALESCE($8, deleted),
                 updated_at     = COALESCE($9, updated_at)
             WHERE id = $1",
        )
        .bind(spot_id)
        .bind(patch.title)
        .bind(patch.description)
        .bind(patch.price_per_hour)
        .bind(patch.images)
        .bind(patch.availability.map(sqlx::types::Json))
        .bind(patch.active)
        .bind(patch.deleted)
        .bind(patch.updated_at)
        .execute(ex)
        .await?;
        Ok(())
    }
}
