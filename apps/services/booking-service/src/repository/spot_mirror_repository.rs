use shared::domain_models::booking::{SpotMirror, SpotMirrorPatch};
use shared::error::myerror::MyResult;
use sqlx::PgExecutor;
use uuid::Uuid;

/// booking-service's local mirror of the `spot` table.
///
/// Three statements: the plain read, the read that **locks**, and the merge.
pub struct SpotMirrorRepository;

impl SpotMirrorRepository {
    /// `SELECT *`.
    ///
    /// This used to spell out `record::id(id) AS id`, `deleted ?? false` and
    /// `bookings_seq ?? 0`, because the id was a record key and — on this table
    /// especially — a partially-built row was the normal cold-rebuild case rather than
    /// an edge one. The columns that can legitimately be absent are `NULL`-able in the
    /// schema and `Option` on the model, so the read needs no defaults.
    pub async fn find_by_id(
        ex: impl PgExecutor<'_>,
        spot_id: Uuid,
    ) -> MyResult<Option<SpotMirror>> {
        Ok(sqlx::query_as("SELECT * FROM spot WHERE id = $1")
            .bind(spot_id)
            .fetch_optional(ex)
            .await?)
    }

    /// The same read, holding a row lock until the transaction ends.
    ///
    /// # This is the serialisation point for double-booking
    ///
    /// Two renters racing one slot insert two *different* booking rows — different
    /// keys, nothing collides — so without something to contend on, both commit and
    /// the slot is sold twice.
    ///
    /// Under TiKV that something was a counter: `bookings_seq` on this row, bumped
    /// inside the reserve transaction purely to force a write-write conflict the store
    /// would refuse. **Read Committed does not refuse it** — the second writer blocks,
    /// re-reads and applies — so the counter would have gone on being bumped while
    /// silently protecting nothing.
    ///
    /// The lock replaces it, and it works because of what Read Committed does *after*
    /// the wait: each statement takes a **new snapshot**. T2 blocks here until T1
    /// commits, and T2's next statement — `BookingRepository::taken_for_spot` — then
    /// sees T1's booking and refuses the slot with a clean 409 naming it. No retry, no
    /// counter, and the answer the renter gets is the useful one.
    ///
    /// Two things this depends on, both measured on
    /// `yugabytedb/yugabyte:2025.2.5.2-b5` before it was written:
    ///
    ///   - **`yb_enable_read_committed_isolation` must be true**, which it is by
    ///     default only from v2025.2 and only when deployed through `yugabyted`. Below
    ///     that, Read Committed silently degrades to Snapshot: the lock is still taken,
    ///     the wait still happens, and T2 then reads its *original* snapshot — which
    ///     does not contain T1's booking. It double-books, with no error anywhere.
    ///   - **The availability read must come after this call**, not before. A snapshot
    ///     taken ahead of the lock is stale no matter what is locked afterwards.
    ///
    /// See the notes in `docker/docker-compose-dev.yml` and
    /// `migrations/booking/0001_init.sql`.
    pub async fn find_for_update(
        ex: impl PgExecutor<'_>,
        spot_id: Uuid,
    ) -> MyResult<Option<SpotMirror>> {
        Ok(sqlx::query_as("SELECT * FROM spot WHERE id = $1 FOR UPDATE")
            .bind(spot_id)
            .fetch_optional(ex)
            .await?)
    }

    /// Apply a SPOTS event to the mirror, creating the row if it is not there yet.
    ///
    /// Upsert, not update: the row may not exist when a SPOTS edit arrives, because
    /// the streams expire and a consumer built later can see a `SpotUpdated` whose
    /// `SpotCreated` has already aged out.
    ///
    /// `COALESCE($n, column)` means absent-is-unchanged. On a row being created that
    /// resolves to `NULL`, so the table's own defaults decide the rest.
    ///
    /// This no longer has a second job. It was `merge` and never a whole-row write
    /// specifically because `CONTENT` would have erased `bookings_seq`, a column this
    /// stream does not own — that column is gone, so the only reason left is the
    /// partial-row case above.
    ///
    /// **The binds are positional**, so their order must match the `$n`.
    pub async fn merge(
        ex: impl PgExecutor<'_>,
        spot_id: Uuid,
        patch: SpotMirrorPatch,
    ) -> MyResult<()> {
        sqlx::query(
            "INSERT INTO spot (id, owner_id, price_per_hour, availability, timezone, active, deleted)
                  VALUES ($1, $2, $3, $4, $5, COALESCE($6, true), COALESCE($7, false))
             ON CONFLICT (id) DO UPDATE SET
                 owner_id       = COALESCE(EXCLUDED.owner_id,       spot.owner_id),
                 price_per_hour = COALESCE(EXCLUDED.price_per_hour, spot.price_per_hour),
                 availability   = COALESCE(EXCLUDED.availability,   spot.availability),
                 timezone       = COALESCE(EXCLUDED.timezone,       spot.timezone),
                 active         = COALESCE($6,                      spot.active),
                 deleted        = COALESCE($7,                      spot.deleted)",
        )
        .bind(spot_id)
        .bind(patch.owner_id)
        .bind(patch.price_per_hour)
        .bind(patch.availability.map(sqlx::types::Json))
        .bind(patch.timezone)
        .bind(patch.active)
        .bind(patch.deleted)
        .execute(ex)
        .await?;
        Ok(())
    }

    // `advance` is gone. It bumped `spot.bookings_seq` inside the transaction that
    // wrote a booking row, with `math::max` so a redelivered older message could not
    // rewind it. Its entire purpose was to manufacture a write conflict; under Read
    // Committed there is no conflict to manufacture, and `find_for_update` above is
    // what serialises reserve instead.
}
