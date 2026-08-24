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

    /// Points every row that was waiting for this user at it.
    ///
    /// A spot or a booking may already be sitting in the read model with its link
    /// still NONE, because SPOTS and BOOKINGS advance independently of USERS and
    /// routinely arrive first. This is the other half of the subquery in
    /// `ViewSpotRepository::link_owner` and friends: one resolves forwards when the
    /// user is already there, this one backwards when they are not.
    ///
    /// Scoped `AND … = NONE` so it only ever fills a gap, never repoints a row —
    /// which is what makes it safe to re-run on a replay.
    ///
    /// No `BEGIN`/`COMMIT`: this runs inside the transaction `bus::Tx` already opened
    /// for the event, and nesting one would be a different statement than intended.
    /// The three updates are one statement list in one round trip.
    ///
    /// `payout` is included; the hand-written version this replaced covered only
    /// `spot` and `booking`. A payout whose owner had not been projected kept
    /// `owner = NONE` permanently, because nothing else ever revisited the row.
    pub async fn backfill_links(&self, user_id: &Uuid) -> MyResult<()> {
        self.q
            .q("UPDATE spot    SET owner  = type::record('user', $id)
                    WHERE owner_id  = $id AND owner  = NONE;
                UPDATE booking SET renter = type::record('user', $id)
                    WHERE renter_id = $id AND renter = NONE;
                UPDATE payout  SET owner  = type::record('user', $id)
                    WHERE owner_id  = $id AND owner  = NONE;")
            .bind(("id", *user_id))
            .await?
            .check()?;
        Ok(())
    }
}
