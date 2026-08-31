use chrono::{DateTime, Utc};
use shared::domain_models::user::{RefreshToken, RefreshTokenPatch};
use shared::error::myerror::MyResult;
use sqlx::PgExecutor;

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
        ex: impl PgExecutor<'_>,
        token_hash: String,
    ) -> MyResult<Option<RefreshToken>> {
        Ok(
            sqlx::query_as("SELECT * FROM refresh_token WHERE token_hash = $1")
                .bind(token_hash)
                .fetch_optional(ex)
                .await?,
        )
    }

    /// Insert-or-replace the whole row, keyed by its own id.
    ///
    /// The column list is spelled out — sqlx has no whole-struct write — but unlike
    /// before, that is now true of every table here rather than a special cost this
    /// one paid for its link.
    pub async fn upsert(ex: impl PgExecutor<'_>, token: RefreshToken) -> MyResult<()> {
        sqlx::query(
            "INSERT INTO refresh_token
                 (id, user_id, token_hash, jti, created_at, expires_at, revoked, revoked_reason)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
             ON CONFLICT (id) DO UPDATE SET
                 user_id        = EXCLUDED.user_id,
                 token_hash     = EXCLUDED.token_hash,
                 jti            = EXCLUDED.jti,
                 created_at     = EXCLUDED.created_at,
                 expires_at     = EXCLUDED.expires_at,
                 revoked        = EXCLUDED.revoked,
                 revoked_reason = EXCLUDED.revoked_reason",
        )
        .bind(token.id)
        .bind(token.user_id)
        .bind(token.token_hash)
        .bind(token.jti)
        .bind(token.created_at)
        .bind(token.expires_at)
        .bind(token.revoked)
        .bind(token.revoked_reason)
        .execute(ex)
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
        ex: impl PgExecutor<'_>,
        token_hash: String,
        patch: RefreshTokenPatch,
    ) -> MyResult<()> {
        sqlx::query(
            "UPDATE refresh_token SET
                 revoked        = COALESCE($2, revoked),
                 revoked_reason = COALESCE($3, revoked_reason)
             WHERE token_hash = $1",
        )
        .bind(token_hash)
        .bind(patch.revoked)
        .bind(patch.revoked_reason)
        .execute(ex)
        .await?;
        Ok(())
    }

    /// Expired rows can never be revoked or renewed again, so they are dead weight;
    /// dropped whenever this user's sessions are touched. `before` is the event's own
    /// clock, so every replica deletes exactly the same rows.
    pub async fn delete_expired(ex: impl PgExecutor<'_>, before: DateTime<Utc>) -> MyResult<()> {
        sqlx::query("DELETE FROM refresh_token WHERE expires_at < $1")
            .bind(before)
            .execute(ex)
            .await?;
        Ok(())
    }
}
