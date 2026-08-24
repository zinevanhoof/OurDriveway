use std::sync::Arc;

use shared::db::Querier;
use shared::domain_models::payment::{BookingMirror, BookingMirrorPatch};
use shared::error::myerror::MyResult;
use surrealdb::{Surreal, engine::remote::ws::Client};
use uuid::Uuid;

/// payment-service's local mirror of the `booking` table.
pub struct BookingMirrorRepository<Q: Querier = Arc<Surreal<Client>>> {
    pub q: Q,
}

impl<Q: Querier> BookingMirrorRepository<Q> {
    /// `*` takes every column, so a new field on [`BookingMirror`] needs no edit
    /// here. Two are spelled out because `*` returns them in a shape the struct
    /// cannot deserialize: `id` comes back as the record key `booking:⟨uuid⟩`, and
    /// `booked` is NONE on rows written before that column existed. An explicit
    /// alias beats `*` for the same name in either order — checked against
    /// SurrealDB 3.2.4 rather than assumed.
    pub async fn find_by_id(&self, booking_id: Uuid) -> MyResult<Option<BookingMirror>> {
        Ok(self
            .q
            .q("SELECT record::id(id) AS id, booked ?? {} AS booked, *
                FROM ONLY type::record('booking', $v)")
            .bind(("v", booking_id))
            .await?
            .take(0)?)
    }

    /// Insert-or-replace the whole row.
    ///
    /// `upsert`, not a merge: `BookingCreated` is always the first event for a
    /// booking and BOOKINGS never expires, so this row is only ever created whole —
    /// which is also why nothing on this table is `option<>`.
    pub async fn upsert(&self, booking: BookingMirror) -> MyResult<()> {
        let id = booking.id;
        self.q
            .q("UPSERT type::record('booking', $id) CONTENT $row")
            .bind(("id", id))
            .bind(("row", booking))
            .await?
            .check()?;
        Ok(())
    }

    /// Patch a booking only if it is currently in one of `from`, and clear the hold.
    ///
    /// Two things worth their own statement, both load-bearing:
    ///
    /// - The `WHERE`. Scoped for the same reason booking-service's own projector
    ///   scopes it: a payment landing microseconds before the hold lapses, with the
    ///   sweeper's event arriving second, must not undo the confirmation.
    /// - `hold_until = NONE`. A patch's `?? column` can leave a value alone but never
    ///   clear it, and every transition out of `reserved` ends the hold. Set here
    ///   rather than by each caller, because forgetting it on one arm leaves a
    ///   settled booking that still looks held.
    ///
    /// The `SET` list deliberately omits `hold_until`, so the trailing assignment is
    /// the only one to that column and the two cannot fight. The other three are
    /// every column [`BookingMirrorPatch`] carries — add one there and it has to be
    /// added here too.
    pub async fn transition(
        &self,
        booking_id: Uuid,
        from: &[&str],
        patch: BookingMirrorPatch,
    ) -> MyResult<()> {
        patch
            .bind(
                self.q
                    .q("UPDATE type::record('booking', $v) SET
                            status         = $status         ?? status,
                            cancel_reason  = $cancel_reason  ?? cancel_reason,
                            release_reason = $release_reason ?? release_reason,
                            hold_until     = NONE
                        WHERE status IN $from;")
                    .bind(("v", booking_id))
                    .bind((
                        "from",
                        from.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
                    )),
            )
            .await?
            .check()?;
        Ok(())
    }
}
