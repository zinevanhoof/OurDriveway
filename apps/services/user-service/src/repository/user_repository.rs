use shared::{
    error::myerror::MyResult,
    events::{
        Envelope,
        user::{UserEvent, UserPasswordChanged, UserRegistered, UserUpdated},
    },
};
use surrealdb::{Surreal, engine::remote::ws::Client, types::SurrealValue};
use uuid::Uuid;

/// Reads serve requests; the `apply_*` methods are the projector's, and are the
/// **only** writers. Handlers publish events and never write here.
pub struct UserRepository {
    pub db: Surreal<Client>,
}

/// What login needs: the record key (a uuid, straight from `record::id(id)`), the
/// stored hash to verify against, and the user's
/// shard so the session event lands on the same subject as their other events.
///
/// `email` is here for the profile edit, which has to know whether the submitted
/// address is actually a change before it decides to demand a password.
///
/// `email_verified` gates login. It is read on the same row as the password hash
/// deliberately — one lookup, and no way to check the credential without also
/// having the flag in hand.
#[derive(SurrealValue)]
pub struct UserAuth {
    pub uid: Uuid,
    pub password: String,
    pub shard: String,
    pub email: String,
    pub email_verified: bool,
}

impl UserRepository {
    pub async fn find_for_login(&self, email: &str) -> MyResult<Option<UserAuth>> {
        // Password comparison moved into Rust (auth/password.rs). SurrealQL's
        // crypto::argon2::compare would work, but hashing has to happen in Rust
        // for determinism, so verification lives next to it.
        let found: Option<UserAuth> = self
            .db
            .query("SELECT record::id(id) AS uid, password, shard, email, email_verified FROM ONLY user WHERE email = $email LIMIT 1")
            .bind(("email", email.to_string()))
            .await?
            .take(0)?;
        Ok(found)
    }

    /// The same row, addressed by id instead of email — what every authenticated
    /// write needs, since the JWT carries the id and nothing else. Also the only
    /// way to learn a user's shard without knowing their email.
    pub async fn find_auth_by_id(&self, uid: &Uuid) -> MyResult<Option<UserAuth>> {
        let found: Option<UserAuth> = self
            .db
            .query("SELECT record::id(id) AS uid, password, shard, email, email_verified FROM ONLY type::record('user', $id)")
            .bind(("id", *uid))
            .await?
            .take(0)?;
        Ok(found)
    }

    /// Just the greeting for an email. Its own query rather than a field on
    /// `UserAuth`, which every login path pays for and none of them greets
    /// anybody.
    pub async fn first_name(&self, uid: &Uuid) -> MyResult<String> {
        let found: Option<String> = self
            .db
            .query("SELECT VALUE first_name FROM ONLY type::record('user', $id)")
            .bind(("id", *uid))
            .await?
            .take(0)?;
        Ok(found.unwrap_or_default())
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
            UserEvent::PasswordChanged(e) => self.password_changed(e, at, seq).await,
            UserEvent::EmailVerified { user_id } => self.email_verified(&user_id, at, seq).await,
            // Purely a message to notification-service; nothing here changes. The
            // cursor still has to move, or a restart replays from before it.
            UserEvent::VerificationRequested(_) => self.bump_cursor(at, seq).await,
        }
    }

    /// Advances the cursor for an event that changes no rows here.
    async fn bump_cursor(&self, at: chrono::DateTime<chrono::Utc>, seq: u64) -> MyResult<()> {
        self.db
            .query("UPSERT _projection:USERS SET last_seq = $seq, updated_at = $at")
            .bind(("at", surrealdb::types::Datetime::from(at)))
            .bind(("seq", seq as i64))
            .await?
            .check()?;
        Ok(())
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
                     email: $email, password: $password, profile_picture: NONE,
                     license_plates: [], email_verified: false
                 };
                 UPSERT _projection:USERS SET last_seq = $seq, updated_at = $at;
                 COMMIT;",
            )
            .bind(("id", e.user_id))
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
                // Two statements, and the order is the point: the address has to
                // be compared against the stored one *before* it is overwritten.
                // Fold them into one SET and the comparison reads whatever the
                // engine happened to assign first.
                //
                // Without this, changing to an unverified address keeps the flag
                // from the old one and login lets it straight through — which
                // makes the whole feature decorative.
                "BEGIN;
                 UPDATE type::record('user', $id) SET email_verified = false
                     WHERE $email != NONE AND email != $email;
                 UPDATE type::record('user', $id) SET
                     first_name      = $first_name      ?? first_name,
                     last_name       = $last_name       ?? last_name,
                     profile_picture = $profile_picture ?? profile_picture,
                     email           = $email           ?? email,
                     license_plates  = $license_plates  ?? license_plates;
                 UPSERT _projection:USERS SET last_seq = $seq, updated_at = $at;
                 COMMIT;",
            )
            .bind(("id", e.user_id))
            .bind(("first_name", e.first_name))
            .bind(("last_name", e.last_name))
            .bind(("profile_picture", e.profile_picture))
            .bind(("email", e.email))
            .bind(("license_plates", e.license_plates))
            .bind(("at", surrealdb::types::Datetime::from(at)))
            .bind(("seq", seq as i64))
            .await?
            .check()?;
        Ok(())
    }

    /// Idempotent by construction — setting `true` twice is setting `true`. That
    /// matters because mail scanners prefetch links, so the endpoint that
    /// publishes this event is deliberately re-runnable.
    async fn email_verified(
        &self,
        user_id: &uuid::Uuid,
        at: chrono::DateTime<chrono::Utc>,
        seq: u64,
    ) -> MyResult<()> {
        self.db
            .query(
                "BEGIN;
                 UPDATE type::record('user', $id) SET email_verified = true;
                 UPSERT _projection:USERS SET last_seq = $seq, updated_at = $at;
                 COMMIT;",
            )
            .bind(("id", *user_id))
            .bind(("at", surrealdb::types::Datetime::from(at)))
            .bind(("seq", seq as i64))
            .await?
            .check()?;
        Ok(())
    }

    async fn password_changed(
        &self,
        e: UserPasswordChanged,
        at: chrono::DateTime<chrono::Utc>,
        seq: u64,
    ) -> MyResult<()> {
        self.db
            .query(
                "BEGIN;
                 UPDATE type::record('user', $id) SET password = $password;
                 UPSERT _projection:USERS SET last_seq = $seq, updated_at = $at;
                 COMMIT;",
            )
            .bind(("id", e.user_id))
            .bind(("password", e.password_hash))
            .bind(("at", surrealdb::types::Datetime::from(at)))
            .bind(("seq", seq as i64))
            .await?
            .check()?;
        Ok(())
    }
}
