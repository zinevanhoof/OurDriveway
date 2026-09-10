use diesel::prelude::*;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use shared::domain_models::view::user::{ViewUser, ViewUserPatch};
use shared::error::myerror::MyResult;
use shared::projections::user::AccountProjection;
use shared::schema::view::app_user;
use uuid::Uuid;

/// The `app_user` table in the read model.
///
/// The table is `app_user` and the aggregate is `user`; see `shared::db::table_for`.
pub struct ViewUserRepository;

impl ViewUserRepository {
    /// `GET /api/view/account` — the caller's own profile, `email` included.
    ///
    /// No `WHERE` beyond the key, and that *is* the `account` namespace's predicate: the
    /// row is chosen by a signature-verified claim, so there is no comparison here to get
    /// wrong. It is also why this is the only read that may hold
    /// [`AccountProjection`]'s three scoped columns.
    pub async fn find_for_account(
        conn: &mut AsyncPgConnection,
        user_id: Uuid,
    ) -> MyResult<Option<AccountProjection>> {
        Ok(app_user::table
            .find(user_id)
            .select(AccountProjection::as_select())
            .first(conn)
            .await
            .optional()?)
    }

    /// Insert-or-replace the whole row.
    ///
    /// The struct's fields *are* the columns written, which is what
    /// `no_credential_columns_reach_the_read_model` leans on — nothing reaches this
    /// world-readable table that is not a field on [`ViewUser`].
    pub async fn upsert(conn: &mut AsyncPgConnection, user: ViewUser) -> MyResult<()> {
        diesel::insert_into(app_user::table)
            .values(user.clone())
            .on_conflict(app_user::id)
            .do_update()
            .set(user)
            .execute(conn)
            .await?;
        Ok(())
    }

    /// Update only the columns the patch carries. Does not create the row.
    ///
    /// The six columns here are every column [`ViewUserPatch`] carries. `AsChangeset`
    /// omits an absent field rather than coalescing it — same effect, matched by name.
    pub async fn patch(
        conn: &mut AsyncPgConnection,
        user_id: Uuid,
        patch: ViewUserPatch,
    ) -> MyResult<()> {
        diesel::update(app_user::table.find(user_id))
            .set(&patch)
            .execute(conn)
            .await?;
        Ok(())
    }
}

// `backfill_links` was already gone; now so is the thing it was compensating for.
//
// It pointed every spot, booking and payout that was waiting for a user at them once
// that user arrived — the backwards half of a pair whose forwards half was a
// `SELECT … WHERE record::id(id) = $x` in each of the other repositories. Both halves
// existed because a record link could only be written if its target was already
// projected, and the streams advance independently so it routinely was not.
//
// There are no record links. `host_id`, `spot_id` and `renter_id` are plain uuids
// with no foreign key (deliberately — see `migrations/view/0001_init/up.sql`), and a read
// LEFT JOINs to resolve them. A reference to a row that has not been projected yet is
// an absent join rather than a null column that needs repairing later.
