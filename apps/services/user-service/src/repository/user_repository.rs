use shared::{
    error::myerror::MyResult,
    events::{
        Envelope,
        user::{UserEvent, UserRegistered, UserUpdated, record_key},
    },
};
use surrealdb::{Surreal, engine::remote::ws::Client, types::SurrealValue};

/// Reads serve requests; the `apply_*` methods are the projector's, and are the
/// **only** writers. Handlers publish events and never write here.
pub struct UserRepository {
    pub db: Surreal<Client>,
}

/// What login needs: the record key as a plain uuid (so the JWT claim can be
/// built as `user:<uuid>`), the stored hash to verify against, and the user's
/// shard so the session event lands on the same subject as their other events.
#[derive(SurrealValue)]
pub struct UserAuth {
    pub uid: String,
    pub password: String,
    pub shard: String,
}

impl UserRepository {
    pub async fn find_for_login(&self, email: &str) -> MyResult<Option<UserAuth>> {
        // Password comparison moved into Rust (auth/password.rs). SurrealQL's
        // crypto::argon2::compare would work, but hashing has to happen in Rust
        // for determinism, so verification lives next to it.
        let found: Option<UserAuth> = self
            .db
            .query("SELECT record::id(id) AS uid, password, shard FROM ONLY user WHERE email = $email LIMIT 1")
            .bind(("email", email.to_string()))
            .await?
            .take(0)?;
        Ok(found)
    }

    pub async fn email_taken(&self, email: &str) -> MyResult<bool> {
        let found: Option<String> = self
            .db
            .query("SELECT VALUE email FROM ONLY user WHERE email = $email LIMIT 1")
            .bind(("email", email.to_string()))
            .await?
            .take(0)?;
        Ok(found.is_some())
    }

    // ─── projector side ─────────────────────────────────────────────────────

    pub async fn last_seq(&self) -> MyResult<u64> {
        let seq: Option<i64> = self
            .db
            .query("SELECT VALUE last_seq FROM ONLY _projection:USERS")
            .await?
            .take(0)?;
        Ok(seq.unwrap_or(0).max(0) as u64)
    }

    pub async fn apply(&self, envelope: Envelope<UserEvent>, seq: u64) -> MyResult<()> {
        let at = envelope.occurred_at;
        match envelope.payload {
            UserEvent::Registered(e) => self.registered(e, at, seq).await,
            UserEvent::Updated(e) => self.updated(e, at, seq).await,
        }
    }

    async fn registered(
        &self,
        e: UserRegistered,
        at: chrono::DateTime<chrono::Utc>,
        seq: u64,
    ) -> MyResult<()> {
        self.db
            .query(
                "BEGIN;
                 UPSERT type::record('user', $id) CONTENT {
                     shard: $shard, first_name: $first_name, last_name: $last_name,
                     email: $email, password: $password, profile_picture: NONE
                 };
                 UPSERT _projection:USERS SET last_seq = $seq, updated_at = $at;
                 COMMIT;",
            )
            .bind(("id", record_key(&e.user_id)))
            .bind(("shard", e.shard))
            .bind(("first_name", e.first_name))
            .bind(("last_name", e.last_name))
            .bind(("email", e.email))
            .bind(("password", e.password_hash))
            .bind(("at", surrealdb::types::Datetime::from(at)))
            .bind(("seq", seq as i64))
            .await?
            .check()?;
        Ok(())
    }

    async fn updated(
        &self,
        e: UserUpdated,
        at: chrono::DateTime<chrono::Utc>,
        seq: u64,
    ) -> MyResult<()> {
        self.db
            .query(
                "BEGIN;
                 UPDATE type::record('user', $id) SET
                     first_name      = $first_name      ?? first_name,
                     last_name       = $last_name       ?? last_name,
                     profile_picture = $profile_picture ?? profile_picture;
                 UPSERT _projection:USERS SET last_seq = $seq, updated_at = $at;
                 COMMIT;",
            )
            .bind(("id", record_key(&e.user_id)))
            .bind(("first_name", e.first_name))
            .bind(("last_name", e.last_name))
            .bind(("profile_picture", e.profile_picture))
            .bind(("at", surrealdb::types::Datetime::from(at)))
            .bind(("seq", seq as i64))
            .await?
            .check()?;
        Ok(())
    }
}

