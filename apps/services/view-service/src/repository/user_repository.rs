use std::sync::Arc;

use shared::db::Querier;
use shared::domain_models::view::user::{ViewUser, ViewUserPatch};
use shared::error::myerror::MyResult;
use surrealdb::{Surreal, engine::remote::ws::Client};
use uuid::Uuid;

/// The `user` table in the read model.
///
/// The only one of the four with no link column of its own, so a whole-row
/// `CONTENT $row` is safe here — nothing on this row belongs to another stream, and
/// it is also the only one of the four ever read back whole.
pub struct ViewUserRepository<Q: Querier = Arc<Surreal<Client>>> {
    pub q: Q,
}

impl<Q: Querier> ViewUserRepository<Q> {
    /// The `/me` handler's read, and the only whole-row read in this service.
    ///
    /// `*` is safe here precisely because this table has no link column: on `spot`,
    /// `booking` or `payout` it would return `owner`/`spot`/`renter`, which those
    /// structs deliberately do not carry.
    pub async fn find_by_id(&self, user_id: Uuid) -> MyResult<Option<ViewUser>> {
        Ok(self
            .q
            .q("SELECT record::id(id) AS id,
                       license_plates ?? [] AS license_plates,
                       *
                FROM ONLY type::record('user', $v)")
            .bind(("v", user_id))
            .await?
            .take(0)?)
    }

    /// Insert-or-replace the whole row.
    ///
    /// Safe as `CONTENT` here and nowhere else in this service: no link column, and
    /// no column another stream owns. The struct's fields *are* the columns written,
    /// which is what `no_credential_columns_reach_the_read_model` leans on.
    pub async fn upsert(&self, user: ViewUser) -> MyResult<()> {
        let id = user.id;
        self.q
            .q("UPSERT type::record('user', $id) CONTENT $row")
            .bind(("id", id))
            .bind(("row", user))
            .await?
            .check()?;
        Ok(())
    }

    /// Update only the columns the patch carries. Does not create the row.
    ///
    /// The five columns here are every column [`ViewUserPatch`] carries — add one
    /// there and it has to be added here too.
    pub async fn patch(&self, user_id: Uuid, patch: ViewUserPatch) -> MyResult<()> {
        patch
            .bind(
                self.q
                    .q("UPDATE type::record('user', $v) SET
                            first_name      = $first_name      ?? first_name,
                            last_name       = $last_name       ?? last_name,
                            profile_picture = $profile_picture ?? profile_picture,
                            email           = $email           ?? email,
                            license_plates  = $license_plates  ?? license_plates;")
                    .bind(("v", user_id)),
            )
            .await?
            .check()?;
        Ok(())
    }

}

// `backfill_links` is gone. It pointed every spot, booking and payout that was
// waiting for a user at them once that user arrived — the backwards half of a pair
// whose forwards half was a `SELECT … WHERE record::id(id) = $x` in each of the other
// repositories.
//
// Both halves existed to work around the same thing: a link written only if its
// target was already projected. Those links are `option<record<…>>` with no existence
// constraint, so they can simply be written — a link to a row that has not arrived yet
// reads as absent and resolves itself when it does. Writing them straight
// (`ViewBookingRepository::link_refs`) makes the forwards case total, which leaves the
// backwards case with nothing to repair.
//
// It also closed a hole neither half covered: USERS and BOOKINGS advance
// independently, so each could start its transaction before the other's row was
// visible, neither resolve, and the link stay null permanently.
