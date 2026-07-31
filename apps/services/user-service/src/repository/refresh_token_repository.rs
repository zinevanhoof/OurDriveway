use shared::{
    error::myerror::MyResult,
    events::{
        Envelope,
        session::{RefreshTokenIssued, RefreshTokenRevoked, RefreshTokenRotated, SessionEvent},
        user::record_key,
    },
};
use surrealdb::{
    Surreal,
    engine::remote::ws::Client,
    types::{Datetime, SurrealValue},
};

pub struct RefreshTokenRepository {
    pub db: Surreal<Client>,
}

/// What the refresh and logout paths need: the owner's record key as a plain
/// uuid (so the new JWT's `id` claim is built the same way login builds it), the
/// user's shard (so the event goes to the same subject as the rest of that
/// user's sessions), and enough to decide whether the token is still valid.
#[derive(SurrealValue)]
pub struct RefreshTokenAuth {
    pub user_uid: String,
    pub shard: String,
    pub revoked: bool,
    pub expires_at: Datetime,
}

impl RefreshTokenRepository {
    /// Looked up by hash — the plaintext token is never stored.
    pub async fn find_by_hash(&self, token_hash: &str) -> MyResult<Option<RefreshTokenAuth>> {
        let found: Option<RefreshTokenAuth> = self
            .db
            .query(
                "SELECT record::id(user_id) AS user_uid, shard, revoked, expires_at
                 FROM ONLY refresh_token
                 WHERE token_hash = $token_hash
                 LIMIT 1;",
            )
            .bind(("token_hash", token_hash.to_string()))
            .await?
            .take(0)?;

        Ok(found)
    }

    // ─── projector side ─────────────────────────────────────────────────────

    pub async fn last_seq(&self) -> MyResult<u64> {
        let seq: Option<i64> = self
            .db
            .query("SELECT VALUE last_seq FROM ONLY _projection:SESSIONS")
            .await?
            .take(0)?;
        Ok(seq.unwrap_or(0).max(0) as u64)
    }

    pub async fn apply(&self, envelope: Envelope<SessionEvent>, seq: u64) -> MyResult<()> {
        let at = envelope.occurred_at;
        match envelope.payload {
            SessionEvent::Issued(e) => self.issued(e, at, seq).await,
            SessionEvent::Rotated(e) => self.rotated(e, at, seq).await,
            SessionEvent::Revoked(e) => self.revoked(e, at, seq).await,
        }
    }

    async fn issued(
        &self,
        e: RefreshTokenIssued,
        at: chrono::DateTime<chrono::Utc>,
        seq: u64,
    ) -> MyResult<()> {
        self.db
            .query(
                "BEGIN;
                 UPSERT type::record('refresh_token', $id) CONTENT {
                     user_id: type::record('user', $user_id), shard: $shard,
                     token_hash: $token_hash, jti: $jti,
                     created_at: $at, expires_at: $expires_at,
                     revoked: false, revoked_reason: NONE
                 };
                 UPSERT _projection:SESSIONS SET last_seq = $seq, updated_at = $at;
                 COMMIT;",
            )
            .bind(("id", record_key(&e.token_id)))
            .bind(("user_id", record_key(&e.user_id)))
            .bind(("shard", e.shard))
            .bind(("token_hash", e.token_hash))
            .bind(("jti", surrealdb::types::Uuid::from(e.jti)))
            .bind(("expires_at", Datetime::from(e.expires_at)))
            .bind(("at", Datetime::from(at)))
            .bind(("seq", seq as i64))
            .await?
            .check()?;
        Ok(())
    }

    /// Revoke-and-issue in one transaction, mirroring the single event.
    async fn rotated(
        &self,
        e: RefreshTokenRotated,
        at: chrono::DateTime<chrono::Utc>,
        seq: u64,
    ) -> MyResult<()> {
        self.db
            .query(
                "BEGIN;
                 UPDATE refresh_token SET revoked = true, revoked_reason = 'Rotation'
                     WHERE token_hash = $old_hash;
                 UPSERT type::record('refresh_token', $id) CONTENT {
                     user_id: type::record('user', $user_id), shard: $shard,
                     token_hash: $token_hash, jti: $jti,
                     created_at: $at, expires_at: $expires_at,
                     revoked: false, revoked_reason: NONE
                 };
                 UPSERT _projection:SESSIONS SET last_seq = $seq, updated_at = $at;
                 COMMIT;",
            )
            .bind(("old_hash", e.old_token_hash))
            .bind(("id", record_key(&e.token_id)))
            .bind(("user_id", record_key(&e.user_id)))
            .bind(("shard", e.shard))
            .bind(("token_hash", e.token_hash))
            .bind(("jti", surrealdb::types::Uuid::from(e.jti)))
            .bind(("expires_at", Datetime::from(e.expires_at)))
            .bind(("at", Datetime::from(at)))
            .bind(("seq", seq as i64))
            .await?
            .check()?;
        Ok(())
    }

    async fn revoked(
        &self,
        e: RefreshTokenRevoked,
        at: chrono::DateTime<chrono::Utc>,
        seq: u64,
    ) -> MyResult<()> {
        self.db
            .query(
                "BEGIN;
                 UPDATE refresh_token SET revoked = true, revoked_reason = $reason
                     WHERE token_hash = $token_hash;
                 -- Expired rows can never be revoked or renewed again, so they are
                 -- dead weight; drop them whenever this user's sessions are touched.
                 -- `$at` is the event's own clock, so every replica deletes exactly
                 -- the same rows.
                 DELETE refresh_token WHERE expires_at < $at;
                 UPSERT _projection:SESSIONS SET last_seq = $seq, updated_at = $at;
                 COMMIT;",
            )
            .bind(("token_hash", e.token_hash))
            .bind(("reason", e.reason))
            .bind(("at", Datetime::from(at)))
            .bind(("seq", seq as i64))
            .await?
            .check()?;
        Ok(())
    }
}
