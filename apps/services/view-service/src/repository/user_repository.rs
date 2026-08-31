use shared::domain_models::view::user::{ViewUser, ViewUserPatch};
use shared::error::myerror::MyResult;
use shared::projections::user::OwnerViewUser;
use sqlx::PgExecutor;
use uuid::Uuid;

/// The `app_user` table in the read model.
///
/// The table is `app_user` and the aggregate is `user`; see `shared::db::table_for`.
pub struct ViewUserRepository;

impl ViewUserRepository {
    /// The `/me` handler's read: the caller's own profile, `email` included.
    ///
    /// The columns are aliased `user_*` even though nothing is joined here. That is the
    /// one convention `PublicViewUser` decodes by — see `shared::projections` — and it
    /// is what lets the same type serve this read, owner-on-spot and renter-on-booking
    /// without a per-site variant.
    ///
    /// No `WHERE` beyond the key: the row is chosen by a signature-verified claim, so
    /// there is no comparison here to get wrong.
    pub async fn find_owner_by_id(
        ex: impl PgExecutor<'_>,
        user_id: Uuid,
    ) -> MyResult<Option<OwnerViewUser>> {
        Ok(sqlx::query_as(
            "SELECT id              AS user_id,
                    first_name      AS user_first_name,
                    last_name       AS user_last_name,
                    profile_picture AS user_profile_picture,
                    email           AS user_email,
                    license_plates  AS user_license_plates
               FROM app_user
              WHERE id = $1",
        )
        .bind(user_id)
        .fetch_optional(ex)
        .await?)
    }

    /// Insert-or-replace the whole row.
    ///
    /// The struct's fields *are* the columns written, which is what
    /// `no_credential_columns_reach_the_read_model` leans on — nothing reaches this
    /// world-readable table that is not a field on [`ViewUser`].
    pub async fn upsert(ex: impl PgExecutor<'_>, user: ViewUser) -> MyResult<()> {
        sqlx::query(
            "INSERT INTO app_user
                 (id, version, first_name, last_name, profile_picture, email, license_plates)
             VALUES ($1, $2, $3, $4, $5, $6, $7)
             ON CONFLICT (id) DO UPDATE SET
                 version         = EXCLUDED.version,
                 first_name      = EXCLUDED.first_name,
                 last_name       = EXCLUDED.last_name,
                 profile_picture = EXCLUDED.profile_picture,
                 email           = EXCLUDED.email,
                 license_plates  = EXCLUDED.license_plates",
        )
        .bind(user.id)
        .bind(user.version as i64)
        .bind(user.first_name)
        .bind(user.last_name)
        .bind(user.profile_picture)
        .bind(user.email)
        .bind(user.license_plates)
        .execute(ex)
        .await?;
        Ok(())
    }

    /// Update only the columns the patch carries. Does not create the row.
    ///
    /// The five columns here are every column [`ViewUserPatch`] carries.
    /// **The binds are positional**, so their order must match the `$n`.
    pub async fn patch(
        ex: impl PgExecutor<'_>,
        user_id: Uuid,
        patch: ViewUserPatch,
    ) -> MyResult<()> {
        sqlx::query(
            "UPDATE app_user SET
                 first_name      = COALESCE($2, first_name),
                 last_name       = COALESCE($3, last_name),
                 profile_picture = COALESCE($4, profile_picture),
                 email           = COALESCE($5, email),
                 license_plates  = COALESCE($6, license_plates)
             WHERE id = $1",
        )
        .bind(user_id)
        .bind(patch.first_name)
        .bind(patch.last_name)
        .bind(patch.profile_picture)
        .bind(patch.email)
        .bind(patch.license_plates)
        .execute(ex)
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
// There are no record links. `owner_id`, `spot_id` and `renter_id` are plain uuids
// with no foreign key (deliberately — see `migrations/view/0001_init.sql`), and a read
// LEFT JOINs to resolve them. A reference to a row that has not been projected yet is
// an absent join rather than a null column that needs repairing later.
