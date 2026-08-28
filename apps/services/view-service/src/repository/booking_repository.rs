use std::sync::Arc;

use shared::db::Querier;
use shared::domain_models::view::booking::{ViewBooking, ViewBookingPatch};
use shared::error::myerror::MyResult;
use surrealdb::types::vars;
use surrealdb::{Surreal, engine::remote::ws::Client};
use uuid::Uuid;

/// The `booking` table in the read model.
pub struct ViewBookingRepository<Q: Querier = Arc<Surreal<Client>>> {
    pub q: Q,
}

impl<Q: Querier> ViewBookingRepository<Q> {
    /// Insert-or-replace the whole row.
    ///
    /// `CONTENT` clears the `spot` and `renter` links, which
    /// [`ViewBookingRepository::link_refs`] puts back in the same transaction. Both
    /// resolve to NONE when their targets have not been projected yet, so neither
    /// can be written from the row shape.
    pub async fn upsert(&self, booking: ViewBooking) -> MyResult<()> {
        let id = booking.id;
        self.q
            .q("UPSERT type::record('booking', $id) CONTENT $row")
            .bind(("id", id))
            .bind(("row", booking))
            .await?
            .check()?;
        Ok(())
    }

    /// Points `spot` and `renter` at their rows.
    ///
    /// The string ids beside them are what permissions and filters actually use — the
    /// links exist only so a GraphQL query can traverse into a spot or a renter
    /// without a second round trip.
    ///
    /// **Written unconditionally, not resolved through a `SELECT … WHERE record::id`.**
    /// The schema types these `option<record<…>>` with no existence constraint, so a
    /// link to a row that has not been projected yet is legal and reads as absent —
    /// exactly what the subquery's NONE produced — and then becomes correct by itself
    /// the moment that row arrives.
    ///
    /// The subquery version left the link NONE *permanently* whenever the target had
    /// not landed yet. That was patched over by `backfill_links` methods that ran when
    /// the target arrived, which covered the sequential cases and not the concurrent
    /// one: USERS and BOOKINGS advance independently, so each side could start its
    /// transaction before the other's row was visible, neither resolve, and the link
    /// stay null for good. Measured, on a rebuild that applied both streams at once.
    ///
    /// Also unconditional rather than scoped `= NONE`: this runs straight after the
    /// `upsert` that cleared both, in the same transaction, so there is never an
    /// existing link here to preserve.
    pub async fn link_refs(
        &self,
        booking_id: &Uuid,
        spot_id: &Uuid,
        renter_id: &Uuid,
    ) -> MyResult<()> {
        self.q
            .q("UPDATE type::record('booking', $id) SET
                    spot   = type::record('spot', $spot),
                    renter = type::record('user', $renter);")
            .bind(vars! {
                id:     *booking_id,
                spot:   *spot_id,
                renter: *renter_id,
            })
            .await?
            .check()?;
        Ok(())
    }

    /// Settles a booking, but only out of the status the event is allowed to leave.
    ///
    /// The `WHERE` is the whole guard: a payment landing microseconds before a hold
    /// lapses, with the sweeper's expiry arriving second, must not undo the
    /// confirmation. And `hold_until = NONE` is a *clear*, which a patch's
    /// `?? column` can express only as "leave alone" — so it is assigned here and is
    /// deliberately not a field on [`ViewBookingPatch`], which means the two cannot
    /// fight over it.
    ///
    /// `from` is a parameter because a cancel leaves `confirmed`, not `reserved`, and
    /// hardcoding one would drop the other **silently**. Matching nothing is a
    /// legitimate no-op, not an error, and there is nothing to report either way —
    /// this row *is* the availability answer, so no second copy needs recomputing.
    pub async fn settle(
        &self,
        booking_id: Uuid,
        from: &str,
        patch: ViewBookingPatch,
    ) -> MyResult<()> {
        patch
            .bind(
                self.q
                    .q("UPDATE type::record('booking', $v) SET
                            status         = $status         ?? status,
                            release_reason = $release_reason ?? release_reason,
                            cancel_reason  = $cancel_reason  ?? cancel_reason,
                            hold_until     = NONE
                        WHERE status = $from;")
                    .bind(("v", booking_id))
                    .bind(("from", from.to_string())),
            )
            .await?
            .check()?;
        Ok(())
    }
}
