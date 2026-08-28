use std::sync::Arc;

use shared::db::Querier;
use shared::domain_models::user::{User, UserPatch};
use shared::error::myerror::MyResult;
use surrealdb::{Surreal, engine::remote::ws::Client};
use uuid::Uuid;

/// The `user` table.
///
/// Four statements: two lookups, the write, and the partial update.
///
/// Defaults to the shared `Arc` connection, so every long-lived repository in the
/// process is one refcount bump rather than one session and one root sign-in
/// each. The projector instead builds `UserRepository<&Transaction<Client>>` per
/// event, over the transaction it is already inside.
pub struct UserRepository<Q: Querier = Arc<Surreal<Client>>> {
    pub q: Q,
}

impl<Q: Querier> UserRepository<Q> {
    /// `*` takes every column, so a new field on [`User`] needs no edit here.
    /// Three are spelled out because `*` returns them in a shape the struct cannot
    /// deserialize: `id` is the record key `user:⟨uuid⟩` where the struct holds a
    /// plain uuid, and `license_plates`/`email_verified` are NONE on rows written
    /// before those columns existed.
    ///
    /// An explicit alias beats `*` for the same name in either order — checked
    /// against SurrealDB 3.2.4 rather than assumed.
    pub async fn find_by_id(&self, user_id: Uuid) -> MyResult<Option<User>> {
        Ok(self
            .q
            .q("SELECT record::id(id) AS id,
                       version        ?? 0     AS version,
                       license_plates ?? []    AS license_plates,
                       email_verified ?? false AS email_verified,
                       *
                FROM ONLY type::record('user', $v)")
            .bind(("v", user_id))
            .await?
            .take(0)?)
    }

    /// `email_idx … UNIQUE` in `schemas/user-schema.surql`, so `LIMIT 1` here is a
    /// fact about the schema and not a hope about the data.
    pub async fn find_by_email(&self, email: String) -> MyResult<Option<User>> {
        Ok(self
            .q
            .q("SELECT record::id(id) AS id,
                       version        ?? 0     AS version,
                       license_plates ?? []    AS license_plates,
                       email_verified ?? false AS email_verified,
                       *
                FROM ONLY user WHERE email = $v LIMIT 1")
            .bind(("v", email))
            .await?
            .take(0)?)
    }

    /// Every user, for `UserService::backfill`.
    ///
    /// ponytail: reads the whole table into memory in one pass. Fine for a
    /// maintenance endpoint on a table of accounts; page on `id` — `WHERE id > $after
    /// ORDER BY id LIMIT $n` — if one ever gets big enough to notice.
    pub async fn all(&self) -> MyResult<Vec<User>> {
        Ok(self
            .q
            .q("SELECT record::id(id) AS id,
                       version        ?? 0     AS version,
                       license_plates ?? []    AS license_plates,
                       email_verified ?? false AS email_verified,
                       *
                FROM user")
            .await?
            .take(0)?)
    }

    /// Insert-or-replace the whole row, keyed by its own id.
    ///
    /// UPSERT rather than CREATE: replay must be idempotent, and a
    /// database-generated id would differ per replica. `CONTENT $row` binds the
    /// struct whole, so adding a field to [`User`] needs no change here.
    ///
    /// The row carries its own `id` and the statement also names one. SurrealDB
    /// requires them to agree and errors if they do not, which makes this a free
    /// assertion rather than a risk.
    pub async fn upsert(&self, user: User) -> MyResult<()> {
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
    /// `?? column` means absent-is-unchanged, and is also the ceiling: no patch can
    /// set a column back to NONE. The seven columns here are every column
    /// [`UserPatch`] carries — add one there and it has to be added here too.
    pub async fn patch(&self, user_id: Uuid, patch: UserPatch) -> MyResult<()> {
        patch
            .bind(
                self.q
                    .q("UPDATE type::record('user', $v) SET
                            first_name      = $first_name      ?? first_name,
                            last_name       = $last_name       ?? last_name,
                            email           = $email           ?? email,
                            password        = $password        ?? password,
                            profile_picture = $profile_picture ?? profile_picture,
                            license_plates  = $license_plates  ?? license_plates,
                            email_verified  = $email_verified  ?? email_verified;")
                    .bind(("v", user_id)),
            )
            .await?
            .check()?;
        Ok(())
    }
}
