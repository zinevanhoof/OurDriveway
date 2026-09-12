use diesel::prelude::*;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use shared::domain_models::user::{User, UserPatch};
use shared::error::myerror::MyResult;
use shared::schema::user::app_user;
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
/// to be generic over which one it carried.
///
/// Every method now takes `&mut AsyncPgConnection`, which covers both positions
/// without a trait of ours: a pooled connection derefs to it, and
/// `conn.transaction(|conn| …)` hands back the same type. That is narrower than sqlx's
/// `impl PgExecutor<'_>`, which also accepted `&PgPool` — so a caller holding only a
/// pool now checks a connection out first, and the fact that a method runs inside
/// someone's transaction is visible in its signature rather than implied.
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
    pub async fn find_by_id(conn: &mut AsyncPgConnection, user_id: Uuid) -> MyResult<Option<User>> {
        Ok(app_user::table
            .find(user_id)
            .select(User::as_select())
            .first(conn)
            .await
            .optional()?)
    }

    /// `app_user_email_idx … UNIQUE`, so at most one row can match — a fact about the
    /// schema rather than a hope about the data, and the reason this returns an
    /// `Option` rather than taking the first of a list.
    pub async fn find_by_email(
        conn: &mut AsyncPgConnection,
        email: String,
    ) -> MyResult<Option<User>> {
        Ok(app_user::table
            .filter(app_user::email.eq(email))
            .select(User::as_select())
            .first(conn)
            .await
            .optional()?)
    }

    /// Every user, for `UserService::backfill`.
    ///
    /// ponytail: reads the whole table into memory in one pass. Fine for a
    /// maintenance endpoint on a table of accounts; page on `id` — `WHERE id > $after
    /// ORDER BY id LIMIT $n` — if one ever gets big enough to notice.
    pub async fn all(conn: &mut AsyncPgConnection) -> MyResult<Vec<User>> {
        Ok(app_user::table.select(User::as_select()).load(conn).await?)
    }

    /// Insert-or-replace the whole row, keyed by its own id.
    ///
    /// Upsert rather than insert: replay must be idempotent, and a database-generated
    /// id would differ per replica.
    ///
    /// **The column list is gone.** `#[derive(Insertable)]` on the model binds the
    /// struct whole, which is what `UPSERT … CONTENT $row` did and what sqlx could not
    /// — the three copies of the column list this used to carry (VALUES, and `EXCLUDED`
    /// twice) were the thing most likely to go stale when [`User`] grew a field.
    ///
    /// `.set(&user)` on the conflict branch reuses the same `AsChangeset`, so the
    /// insert and the update cannot disagree about which columns exist.
    pub async fn upsert(conn: &mut AsyncPgConnection, user: User) -> MyResult<()> {
        // By value, twice, hence the clone. `serialize_as` on `version` means the
        // derives are implemented for the owned `User` and not for `&User` — the
        // conversion has to consume the field. One clone of a small struct per write is
        // the price of not spelling the column list out three times.
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
    /// `AsChangeset` on [`UserPatch`] is absent-is-unchanged: a `None` field is OMITTED
    /// from the `SET` list rather than written back to itself. That is the same
    /// observable behaviour as the `COALESCE($n, column)` this replaces, reached
    /// differently — and it keeps the ceiling, since neither can set a column to NULL.
    ///
    /// **The positional binds are gone with it**, and they were the sharp edge: six of
    /// the seven fields were `Option<String>`, so a swapped pair compiled cleanly and
    /// wrote the wrong column. Fields are matched by name now, so that mistake is a
    /// compile error.
    pub async fn patch(
        conn: &mut AsyncPgConnection,
        user_id: Uuid,
        patch: UserPatch,
    ) -> MyResult<()> {
        diesel::update(app_user::table.find(user_id))
            .set(&patch)
            .execute(conn)
            .await?;
        Ok(())
    }
}
