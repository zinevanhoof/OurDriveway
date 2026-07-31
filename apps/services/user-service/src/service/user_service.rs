use async_nats::jetstream::Context;
use chrono::{DateTime, Duration, Utc};
use shared::error::myerror::{ContextExt, MyError, MyResult};
use shared::events::session::{
    RefreshTokenIssued, RefreshTokenRevoked, RefreshTokenRotated, SessionEvent,
};
use shared::events::user::{UserEvent, UserRegistered, user_claim_id};
use shared::events::{Envelope, session_subject, shard_of, user_subject};
use uuid::Uuid;

use crate::{
    CONFIG,
    auth::{jwt::generate_jwt, password, token},
    repository::{
        refresh_token_repository::RefreshTokenRepository, user_repository::UserRepository,
    },
};

/// Write side. Validates, publishes, and waits for its own projection.
///
/// Nothing here writes to the database — the projectors do, from the same events
/// every other instance consumes.
pub struct UserService {
    pub user_repository: UserRepository,
    pub refresh_token_repository: RefreshTokenRepository,
    pub js: Context,
    /// Waits for this instance's own projection to catch up to a published event
    /// before returning, so a signup immediately followed by a login — or a login
    /// immediately followed by a refresh — sees its own write.
    pub users_applied: tokio::sync::watch::Receiver<u64>,
    pub sessions_applied: tokio::sync::watch::Receiver<u64>,
}

impl UserService {
    pub async fn signup(
        &self,
        first_name: &str,
        last_name: &str,
        email: &str,
        password_plain: &str,
    ) -> MyResult<()> {
        // The unique index on email is the real guard; this check exists to turn
        // the common case into a 409 rather than an opaque projector failure,
        // since the event would already be in the log by then.
        if self.user_repository.email_taken(email).await? {
            return Err(MyError::api(
                axum::http::StatusCode::CONFLICT,
                "Email already registered",
                "An account with that email already exists.",
            ));
        }

        let user_id = Uuid::now_v7();
        let shard = shard_of(&user_id);
        let event = UserEvent::Registered(UserRegistered {
            user_id,
            shard: shard.clone(),
            first_name: first_name.to_string(),
            last_name: last_name.to_string(),
            email: email.to_string(),
            // Hashed here, not in the projection: Argon2 salts randomly, so a
            // projection would produce a different hash on every replica.
            password_hash: password::hash(password_plain)?,
        });

        let seq = bus::publish(
            &self.js,
            user_subject(&shard, &user_id),
            &Envelope::new(event, None),
        )
        .await?;

        await_seq(&self.users_applied, seq).await;
        Ok(())
    }

    pub async fn login(&self, email: &str, password_plain: &str) -> MyResult<(String, String)> {
        let found = self.user_repository.find_for_login(email).await?;

        // Verify even when no user matched, against a throwaway hash, so a
        // missing account and a wrong password take the same time to answer.
        let ok = match &found {
            Some(u) => password::verify(&u.password, password_plain),
            None => {
                password::verify("$argon2id$v=19$m=19456,t=2,p=1$c2FsdHNhbHRzYWx0$0000000000000000000000000000000000000000000", password_plain);
                false
            }
        };
        if !ok {
            return Err(MyError::unauthorized("Unauthorized", "Invalid credentials"));
        }
        let user = found.expect("checked above");
        let user_uuid = parse_uuid(&user.uid)?;

        let refresh_token = Uuid::new_v4();
        let (jwt, expires_at, jti) = self.mint_jwt(&user_uuid)?;

        let seq = self
            .publish_session(
                &user.shard,
                &user_uuid,
                SessionEvent::Issued(RefreshTokenIssued {
                    token_id: Uuid::now_v7(),
                    user_id: user_uuid,
                    shard: user.shard.clone(),
                    // Only the hash travels; the plaintext goes to the client's
                    // cookie and nowhere else.
                    token_hash: token::hash(&refresh_token),
                    jti,
                    expires_at,
                }),
            )
            .await?;

        await_seq(&self.sessions_applied, seq).await;
        Ok((jwt, refresh_token.to_string()))
    }

    pub async fn logout(&self, refresh_token: Uuid) -> MyResult<()> {
        let hash = token::hash(&refresh_token);
        // Unknown token is not an error: logging out something already gone is
        // the desired end state, and saying so would confirm which tokens exist.
        let Some(existing) = self.refresh_token_repository.find_by_hash(&hash).await? else {
            return Ok(());
        };
        let user_uuid = parse_uuid(&existing.user_uid)?;

        let seq = self
            .publish_session(
                &existing.shard,
                &user_uuid,
                SessionEvent::Revoked(RefreshTokenRevoked {
                    token_hash: hash,
                    reason: "logout".to_string(),
                }),
            )
            .await?;

        await_seq(&self.sessions_applied, seq).await;
        Ok(())
    }

    pub async fn refresh(&self, old_refresh_token: Uuid) -> MyResult<(String, String)> {
        let old_hash = token::hash(&old_refresh_token);
        let existing = self
            .refresh_token_repository
            .find_by_hash(&old_hash)
            .await?
            .context_not_found(("Not Found", "Could not find refresh token"))?;

        if existing.revoked || existing.expires_at.into_inner() < Utc::now() {
            Err(MyError::unauthorized(
                "Unauthorized",
                "Invalid refresh token",
            ))?
        }

        let user_uuid = parse_uuid(&existing.user_uid)?;
        let new_refresh = Uuid::new_v4();
        let (jwt, expires_at, jti) = self.mint_jwt(&user_uuid)?;

        // One event, not a revoke followed by an issue: rotation is atomic, so
        // there is no window where the old token is dead and the new one is
        // missing.
        let seq = self
            .publish_session(
                &existing.shard,
                &user_uuid,
                SessionEvent::Rotated(RefreshTokenRotated {
                    old_token_hash: old_hash,
                    token_id: Uuid::now_v7(),
                    user_id: user_uuid,
                    shard: existing.shard.clone(),
                    token_hash: token::hash(&new_refresh),
                    jti,
                    expires_at,
                }),
            )
            .await?;

        await_seq(&self.sessions_applied, seq).await;
        Ok((jwt, new_refresh.to_string()))
    }

    /// Returns `(jwt, refresh_expiry, jti)`.
    fn mint_jwt(&self, user_uuid: &Uuid) -> MyResult<(String, DateTime<Utc>, Uuid)> {
        let now = Utc::now();
        let jti = Uuid::new_v4();
        let jwt = generate_jwt(
            &user_claim_id(user_uuid),
            now + Duration::minutes(CONFIG.jwt_expiration),
            jti,
        )?;
        Ok((
            jwt,
            now + Duration::days(CONFIG.refresh_token_expiration),
            jti,
        ))
    }

    async fn publish_session(
        &self,
        shard: &str,
        user_uuid: &Uuid,
        event: SessionEvent,
    ) -> MyResult<u64> {
        bus::publish(
            &self.js,
            session_subject(shard, user_uuid),
            &Envelope::new(event, Some(user_claim_id(user_uuid))),
        )
        .await
    }
}

/// Blocks until the local projector has applied `seq`, capped so a stalled
/// projector degrades to stale reads instead of hanging the request.
async fn await_seq(rx: &tokio::sync::watch::Receiver<u64>, seq: u64) {
    let mut rx = rx.clone();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while *rx.borrow() < seq {
            if rx.changed().await.is_err() {
                break;
            }
        }
    })
    .await;
}

fn parse_uuid(uid: &str) -> MyResult<Uuid> {
    Uuid::parse_str(uid).map_err(|e| MyError::Bus(format!("bad user id {uid}: {e}")))
}
