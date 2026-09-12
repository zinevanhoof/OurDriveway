use diesel::prelude::*;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use shared::domain_models::booking::{SpotMirror, SpotMirrorPatch};
use shared::error::myerror::MyResult;
use shared::schema::booking::spot;
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
        conn: &mut AsyncPgConnection,
        spot_id: Uuid,
    ) -> MyResult<Option<SpotMirror>> {
        Ok(spot::table
            .find(spot_id)
            .select(SpotMirror::as_select())
            .first(conn)
            .await
            .optional()?)
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
    /// `migrations/booking/0001_init/up.sql`.
    pub async fn find_for_update(
        conn: &mut AsyncPgConnection,
        spot_id: Uuid,
    ) -> MyResult<Option<SpotMirror>> {
        // `.for_update()` is the whole point of this function existing separately from
        // `find_by_id` — it is what serialises two renters racing one slot.
        Ok(spot::table
            .find(spot_id)
            .for_update()
            .select(SpotMirror::as_select())
            .first(conn)
            .await
            .optional()?)
    }

    /// Apply a SPOTS event to the mirror, creating the row if it is not there yet.
    ///
    /// Upsert, not update: the row may not exist when a SPOTS edit arrives, because
    /// the streams expire and a consumer built later can see a `SpotUpdated` whose
    /// `SpotCreated` has already aged out.
    ///
    /// **Absent-is-unchanged, twice over, and both halves are derived.** `Insertable`
    /// omits a `None` field from the insert's column list, and an omitted column is the
    /// only way a table default ever applies — so `active` and `deleted` come back
    /// `true`/`false` on a create that does not name them. `AsChangeset` skips the same
    /// `None` on the conflict path, so a `SpotUpdated` carrying only a title leaves every
    /// mirrored column alone.
    ///
    /// That last case is ordinary, not exotic: `SpotMirrorPatch::updated` mirrors three
    /// columns out of a much wider event, so an **entirely empty patch** is what a
    /// rename produces. Diesel handles it — `do_update()` with nothing to set is a no-op
    /// rather than an error, unlike a bare `update().set()`, which is
    /// `QueryBuilderError(EmptyChangeset)`. Checked against a real database, both paths.
    ///
    /// This no longer has a second job. It was `merge` and never a whole-row write
    /// specifically because `CONTENT` would have erased `bookings_seq`, a column this
    /// stream does not own — that column is gone, so the only reason left is the
    /// partial-row case above.
    pub async fn merge(
        conn: &mut AsyncPgConnection,
        spot_id: Uuid,
        patch: SpotMirrorPatch,
    ) -> MyResult<()> {
        diesel::insert_into(spot::table)
            .values((spot::id.eq(spot_id), &patch))
            .on_conflict(spot::id)
            .do_update()
            .set(&patch)
            .execute(conn)
            .await?;
        Ok(())
    }

    // `advance` is gone. It bumped `spot.bookings_seq` inside the transaction that
    // wrote a booking row, with `math::max` so a redelivered older message could not
    // rewind it. Its entire purpose was to manufacture a write conflict; under Read
    // Committed there is no conflict to manufacture, and `find_for_update` above is
    // what serialises reserve instead.
}
