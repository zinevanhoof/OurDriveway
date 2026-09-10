use chrono::{DateTime, Utc};
use diesel::prelude::*;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use shared::domain_models::user::{RefreshToken, RefreshTokenPatch};
use shared::error::myerror::MyResult;
use shared::schema::user::refresh_token;

/// The `refresh_token` table.
///
/// This used to be the one table in the codebase with a **linked** column: `user_id`
/// was `record<user>` in the schema and a bare `Uuid` on the struct, so every
/// statement here had to bridge that — the read unwrapped with `record::id(user_id)`,
/// the write re-wrapped with `type::record('user', $user_id)`, and the upsert could
/// not use `CONTENT $row` because binding the struct whole would send a uuid where a
/// record was required.
///
/// It is a plain uuid with a real foreign key now. All three statements are ordinary,
/// and the asymmetry that had to be got right in each of them is gone.
pub struct RefreshTokenRepository;

impl RefreshTokenRepository {
    /// `refresh_token_hash_idx … UNIQUE`, so at most one row can match. The only way
    /// a token is ever looked up: refresh and logout both arrive holding a plaintext
    /// token and nothing else.
    pub async fn find_by_token_hash(
        conn: &mut AsyncPgConnection,
        token_hash: String,
    ) -> MyResult<Option<RefreshToken>> {
        Ok(refresh_token::table
            .filter(refresh_token::token_hash.eq(token_hash))
            .select(RefreshToken::as_select())
            .first(conn)
            .await
            .optional()?)
    }

    /// Insert-or-replace the whole row, keyed by its own id.
    ///
    /// `#[derive(Insertable)]` binds the struct whole, so there is no column list here
    /// at all — which is what this table's write looked like before sqlx, and could not
    /// have while `user_id` was a record link.
    pub async fn upsert(conn: &mut AsyncPgConnection, token: RefreshToken) -> MyResult<()> {
        diesel::insert_into(refresh_token::table)
            .values(token.clone())
            .on_conflict(refresh_token::id)
            .do_update()
            .set(token)
            .execute(conn)
            .await?;
        Ok(())
    }

    /// Revoke, addressed by the hash rather than the id — callers hold a token, never
    /// a row id.
    ///
    /// The two columns here are every column [`RefreshTokenPatch`] carries. It cannot
    /// repoint `user_id`, so a partial update can never move a token to another
    /// account.
    pub async fn patch_by_token_hash(
        conn: &mut AsyncPgConnection,
        token_hash: String,
        patch: RefreshTokenPatch,
    ) -> MyResult<()> {
        diesel::update(refresh_token::table.filter(refresh_token::token_hash.eq(token_hash)))
            .set(&patch)
            .execute(conn)
            .await?;
        Ok(())
    }

    /// Expired rows can never be revoked or renewed again, so they are dead weight;
    /// dropped whenever this user's sessions are touched. `before` is the event's own
    /// clock, so every replica deletes exactly the same rows.
    pub async fn delete_expired(
        conn: &mut AsyncPgConnection,
        before: DateTime<Utc>,
    ) -> MyResult<()> {
        diesel::delete(refresh_token::table.filter(refresh_token::expires_at.lt(before)))
            .execute(conn)
            .await?;
        Ok(())
    }
}
