use shared::domain_models::user::{User, UserPatch};
use shared::error::myerror::MyResult;
use sqlx::PgExecutor;
use uuid::Uuid;

/// The `app_user` table.
///
/// Five statements: two lookups, the backfill scan, the write, and the partial
/// update.
///
/// **The table is `app_user`; the aggregate is `user`.** `user` is a reserved word,
/// and an unquoted `FROM user` silently reads the current-user keyword instead of
/// erroring — see `shared::db::table_for`, which owns the mapping.
///
/// Stateless. It used to hold the connection (`UserRepository<Q: Querier>`) because
/// `Surreal<Client>` and `Transaction<Client>` shared no trait, so the repository had
/// to be generic over which one it carried. sqlx's `PgExecutor` is implemented for
/// both `&PgPool` and `&mut PgConnection`, so each method just takes one: a service
/// passes `&self.pool`, a projector passes `&mut *tx`.
pub struct UserRepository;

impl UserRepository {
    /// `SELECT *`, with nothing to unwrap or default.
    ///
    /// Every read of this table used to carry `record::id(id) AS id` plus
    /// `version ?? 0`, `license_plates ?? []` and `email_verified ?? false` — the id
    /// because it was the record key `user:⟨uuid⟩` rather than a column, the other
    /// three because rows written before those fields existed held NONE and would not
    /// deserialize. The columns are `NOT NULL DEFAULT …` now, so there is no absent
    /// case to paper over and `*` is the whole statement.
    pub async fn find_by_id(ex: impl PgExecutor<'_>, user_id: Uuid) -> MyResult<Option<User>> {
        Ok(sqlx::query_as("SELECT * FROM app_user WHERE id = $1")
            .bind(user_id)
            .fetch_optional(ex)
            .await?)
    }

    /// `app_user_email_idx … UNIQUE`, so at most one row can match — a fact about the
    /// schema rather than a hope about the data, and the reason this returns an
    /// `Option` rather than taking the first of a list.
    pub async fn find_by_email(ex: impl PgExecutor<'_>, email: String) -> MyResult<Option<User>> {
        Ok(sqlx::query_as("SELECT * FROM app_user WHERE email = $1")
            .bind(email)
            .fetch_optional(ex)
            .await?)
    }

    /// Every user, for `UserService::backfill`.
    ///
    /// ponytail: reads the whole table into memory in one pass. Fine for a
    /// maintenance endpoint on a table of accounts; page on `id` — `WHERE id > $after
    /// ORDER BY id LIMIT $n` — if one ever gets big enough to notice.
    pub async fn all(ex: impl PgExecutor<'_>) -> MyResult<Vec<User>> {
        Ok(sqlx::query_as("SELECT * FROM app_user").fetch_all(ex).await?)
    }

    /// Insert-or-replace the whole row, keyed by its own id.
    ///
    /// Upsert rather than insert: replay must be idempotent, and a database-generated
    /// id would differ per replica.
    ///
    /// **The columns are spelled out, and that is a real loss against what it
    /// replaced.** `UPSERT … CONTENT $row` bound the struct whole, so adding a field
    /// to [`User`] needed no edit here. sqlx has no whole-struct write, so a new field
    /// means editing this list — in three places, since `EXCLUDED` repeats it. The
    /// compiler does not catch the omission; the round-trip test in this module's
    /// sibling `mod.rs` is what would.
    pub async fn upsert(ex: impl PgExecutor<'_>, user: User) -> MyResult<()> {
        sqlx::query(
            "INSERT INTO app_user
                 (id, version, first_name, last_name, email,
                  email_verified, profile_picture, password, license_plates)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
             ON CONFLICT (id) DO UPDATE SET
                 version         = EXCLUDED.version,
                 first_name      = EXCLUDED.first_name,
                 last_name       = EXCLUDED.last_name,
                 email           = EXCLUDED.email,
                 email_verified  = EXCLUDED.email_verified,
                 profile_picture = EXCLUDED.profile_picture,
                 password        = EXCLUDED.password,
                 license_plates  = EXCLUDED.license_plates",
        )
        .bind(user.id)
        .bind(user.version as i64)
        .bind(user.first_name)
        .bind(user.last_name)
        .bind(user.email)
        .bind(user.email_verified)
        .bind(user.profile_picture)
        .bind(user.password)
        .bind(user.license_plates)
        .execute(ex)
        .await?;
        Ok(())
    }

    /// Update only the columns the patch carries. Does not create the row.
    ///
    /// `COALESCE($n, column)` is absent-is-unchanged, and is also the ceiling: no
    /// patch can set a column back to NULL.
    ///
    /// **The binds are positional, so their order is load-bearing.** Every field of
    /// [`UserPatch`] appears here exactly once, in the order its `$n` appears above.
    /// Six of the seven are `Option<String>`, so a swapped pair compiles cleanly and
    /// writes the wrong column — `set_covers_every_patchable_column` in the model is
    /// the reminder to come here, and the live round-trip is what would catch it.
    pub async fn patch(ex: impl PgExecutor<'_>, user_id: Uuid, patch: UserPatch) -> MyResult<()> {
        sqlx::query(
            "UPDATE app_user SET
                 first_name      = COALESCE($2, first_name),
                 last_name       = COALESCE($3, last_name),
                 email           = COALESCE($4, email),
                 password        = COALESCE($5, password),
                 profile_picture = COALESCE($6, profile_picture),
                 license_plates  = COALESCE($7, license_plates),
                 email_verified  = COALESCE($8, email_verified)
             WHERE id = $1",
        )
        .bind(user_id)
        .bind(patch.first_name)
        .bind(patch.last_name)
        .bind(patch.email)
        .bind(patch.password)
        .bind(patch.profile_picture)
        .bind(patch.license_plates)
        .bind(patch.email_verified)
        .execute(ex)
        .await?;
        Ok(())
    }
}
