use std::sync::Arc;

use chrono::{DateTime, Utc};
use shared::db::Querier;
use shared::domain_models::user::{RefreshToken, RefreshTokenPatch};
use shared::error::myerror::MyResult;
use surrealdb::types::vars;
use surrealdb::{Surreal, engine::remote::ws::Client};

/// The `refresh_token` table. Shares the process's one connection — see
/// [`crate::repository::user_repository::UserRepository`].
///
/// The one table in the codebase with a **linked** column: `user_id` is
/// `record<user>` in the schema and a bare `Uuid` on the struct. Every statement
/// here has to bridge that, which is why the upsert names its columns instead of
/// binding the row whole.
pub struct RefreshTokenRepository<Q: Querier = Arc<Surreal<Client>>> {
    pub q: Q,
}

impl<Q: Querier> RefreshTokenRepository<Q> {
    /// `refresh_token_hash … UNIQUE`, so `LIMIT 1` here is a fact about the schema
    /// and not a hope about the data. The only way a token is ever looked up:
    /// refresh and logout both arrive holding a plaintext token and nothing else.
    ///
    /// Both `id` and `user_id` are unwrapped — the first is the record key, the
    /// second is the link.
    pub async fn find_by_token_hash(&self, token_hash: String) -> MyResult<Option<RefreshToken>> {
        Ok(self
            .q
            .q("SELECT record::id(id)      AS id,
                       record::id(user_id) AS user_id,
                       *
                FROM ONLY refresh_token WHERE token_hash = $v LIMIT 1")
            .bind(("v", token_hash))
            .await?
            .take(0)?)
    }

    /// Insert-or-replace the whole row, keyed by its own id.
    ///
    /// **Not `CONTENT $row`**, unlike every other table here. The struct holds
    /// `user_id` as a bare uuid and the schema declares it `record<user>`, so the
    /// column has to be re-wrapped on the way in — binding the row whole would send
    /// a uuid where a record is required. SCHEMAFULL means that fails loudly rather
    /// than silently, but it still fails.
    ///
    /// The cost is that adding a column to [`RefreshToken`] means editing this
    /// list. That is the price of the link, and it buys a real foreign key inside
    /// one database.
    pub async fn upsert(&self, token: RefreshToken) -> MyResult<()> {
        self.q
            .q("UPSERT type::record('refresh_token', $id) CONTENT {
                    user_id:        type::record('user', $user_id),
                    shard:          $shard,
                    token_hash:     $token_hash,
                    jti:            $jti,
                    created_at:     $created_at,
                    expires_at:     $expires_at,
                    revoked:        $revoked,
                    revoked_reason: $revoked_reason
                }")
            .bind(vars! {
                id:             token.id,
                user_id:        token.user_id,
                shard:          token.shard,
                token_hash:     token.token_hash,
                jti:            token.jti,
                created_at:     token.created_at,
                expires_at:     token.expires_at,
                revoked:        token.revoked,
                revoked_reason: token.revoked_reason,
            })
            .await?
            .check()?;
        Ok(())
    }

    /// Revoke, addressed by the hash rather than the id — callers hold a token,
    /// never a row id.
    ///
    /// The two columns here are every column [`RefreshTokenPatch`] carries. It
    /// cannot repoint `user_id`, which is why the link's asymmetry does not have to
    /// be got right a third time.
    pub async fn patch_by_token_hash(
        &self,
        token_hash: String,
        patch: RefreshTokenPatch,
    ) -> MyResult<()> {
        patch
            .bind(
                self.q
                    .q("UPDATE refresh_token SET
                            revoked        = $revoked        ?? revoked,
                            revoked_reason = $revoked_reason ?? revoked_reason
                        WHERE token_hash = $v;")
                    .bind(("v", token_hash)),
            )
            .await?
            .check()?;
        Ok(())
    }

    /// Expired rows can never be revoked or renewed again, so they are dead
    /// weight; dropped whenever this user's sessions are touched. `before` is the
    /// event's own clock, so every replica deletes exactly the same rows.
    pub async fn delete_expired(&self, before: DateTime<Utc>) -> MyResult<()> {
        self.q
            .q("DELETE refresh_token WHERE expires_at < $before")
            .bind(("before", before))
            .await?
            .check()?;
        Ok(())
    }
}
