use async_nats::jetstream::Context;
use chrono::{DateTime, Duration, Utc};
use shared::error::myerror::{ContextExt, MyError, MyResult};
use shared::events::session::{
    RefreshTokenIssued, RefreshTokenRevoked, RefreshTokenRotated, SessionEvent,
};
use shared::events::user::{
    UserEvent, UserPasswordChanged, UserRegistered, UserUpdated, VerificationRequested,
};
use shared::events::{Envelope, session_subject, shard_of, user_subject};
use shared::requests::user::UpdateProfileRequest;
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
                password::verify(
                    "$argon2id$v=19$m=19456,t=2,p=1$c2FsdHNhbHRzYWx0$0000000000000000000000000000000000000000000",
                    password_plain,
                );
                false
            }
        };
        if !ok {
            return Err(MyError::unauthorized("Unauthorized", "Invalid credentials"));
        }
        let user = found.expect("checked above");

        // After the password check, never before. Answering "verify your email"
        // to an unauthenticated caller would confirm the address is registered,
        // turning the login form into an account-enumeration oracle — the whole
        // reason the branch above burns a hash on a missing user.
        //
        // Distinct title so the client can tell this apart from a wrong password
        // and offer to re-send instead of "check your credentials".
        if !user.email_verified {
            return Err(MyError::api(
                axum::http::StatusCode::FORBIDDEN,
                "Email not verified",
                "Check your inbox for the verification link before logging in.",
            ));
        }

        let user_uuid = user.uid;

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

    /// Marks an address confirmed, given a token only that mailbox received.
    ///
    /// Unauthenticated on purpose: the token *is* the credential. It is verified
    /// under `EMAIL_TOKEN_SECRET` and `Purpose::VerifyEmail`, so it cannot be a
    /// repurposed access token and cannot be a password-reset link.
    ///
    /// **Deliberately idempotent.** Mail scanners prefetch links, and a user who
    /// clicks twice is not an error. Publishing `EmailVerified` a second time is
    /// a no-op in the projection, so there is nothing to guard against — and a
    /// "already used" check here would be the bug, not the fix.
    ///
    /// Mints no session. Proving an address is reachable and authenticating a
    /// person are different claims; a link that logs someone in is exactly what
    /// scanner prefetch turns into an account compromise.
    pub async fn verify_email(&self, token: &str) -> MyResult<()> {
        let user_id = shared::email_token::verify(
            &CONFIG.email_token_secret,
            token,
            shared::email_token::Purpose::VerifyEmail,
        )?;

        // The token proves which account, but not which shard — and events for a
        // user must stay on the subject their history already lives on.
        let user = self
            .user_repository
            .find_auth_by_id(&user_id)
            .await?
            .context_not_found(("Not Found", "Could not find user"))?;

        let seq = self
            .publish_user(&user.shard, &user_id, UserEvent::EmailVerified { user_id })
            .await?;

        await_seq(&self.users_applied, seq).await;
        Ok(())
    }

    /// Asks notification-service to send the verification link again.
    ///
    /// Returns `Ok(())` whether or not the address exists, and whether or not it
    /// is already verified. Anything else makes this an account-enumeration
    /// oracle for an endpoint that needs no credentials at all — the same reason
    /// `logout` shrugs at an unknown token.
    ///
    // ponytail: no rate limit. One event per request, and the event is what costs
    // money to deliver. Add a per-user cooldown (last-sent timestamp on the row,
    // checked here) if this ever gets pointed at.
    pub async fn resend_verification(&self, email: &str) -> MyResult<()> {
        let Some(user) = self.user_repository.find_for_login(email).await? else {
            return Ok(());
        };
        if user.email_verified {
            return Ok(());
        }
        let user_uuid = user.uid;

        // First name comes off the projection rather than the token, because the
        // template greets the reader by it and the token carries only an id.
        let first_name = self.user_repository.first_name(&user.uid).await?;

        let seq = self
            .publish_user(
                &user.shard,
                &user_uuid,
                UserEvent::VerificationRequested(VerificationRequested {
                    user_id: user_uuid,
                    email: user.email,
                    first_name,
                }),
            )
            .await?;

        await_seq(&self.users_applied, seq).await;
        Ok(())
    }

    pub async fn logout(&self, refresh_token: Uuid) -> MyResult<()> {
        let hash = token::hash(&refresh_token);
        // Unknown token is not an error: logging out something already gone is
        // the desired end state, and saying so would confirm which tokens exist.
        let Some(existing) = self.refresh_token_repository.find_by_hash(&hash).await? else {
            return Ok(());
        };
        let user_uuid = existing.user_uid;

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

        let user_uuid = existing.user_uid;
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

    /// Saves the edit-profile form. Returns the log position, which the caller
    /// answers with so the client can wait for *view-service's* projection —
    /// `await_seq` below only covers this service's own.
    ///
    /// No ownership lookup: the target is always the caller's own record.
    pub async fn update_profile(&self, uid: &Uuid, req: UpdateProfileRequest) -> MyResult<u64> {
        let existing = self
            .user_repository
            .find_auth_by_id(uid)
            .await?
            .context_not_found(("Not Found", "Could not find user"))?;
        let user_uuid = existing.uid;

        // Changing the address a password reset would be sent to is an account
        // takeover if it's left unguarded. Verification now catches it afterwards
        // too — the projector clears `email_verified` on any address change, so
        // the account is locked out of login until the new one is confirmed — but
        // this check stays: it stops the takeover instead of merely stranding the
        // victim's account behind a link sent to the attacker. The rest of the
        // form is harmless, so only this branch pays for it.
        if req.email != existing.email {
            let Some(current) = req.current_password.as_deref() else {
                return Err(MyError::api(
                    axum::http::StatusCode::UNPROCESSABLE_ENTITY,
                    "Password required",
                    "Enter your current password to change your email address.",
                ));
            };
            if !password::verify(&existing.password, current) {
                return Err(MyError::unauthorized("Unauthorized", "Incorrect password"));
            }
            // Same as signup: the unique index is the real guard, this only
            // turns the common case into a 409 rather than a projector failure
            // on an event that is already in the log.
            if self.user_repository.email_taken(&req.email).await? {
                return Err(MyError::api(
                    axum::http::StatusCode::CONFLICT,
                    "Email already registered",
                    "An account with that email already exists.",
                ));
            }
        }

        let event = UserEvent::Updated(UserUpdated {
            user_id: user_uuid,
            first_name: Some(req.first_name),
            last_name: Some(req.last_name),
            email: Some(req.email),
            license_plates: Some(req.license_plates),
            // Straight through: the form only sends this when the user picked a
            // new picture, and `None` already means "unchanged" both in the event
            // and in the projection's `?? profile_picture`.
            profile_picture: req.profile_picture,
        });

        let seq = self
            .publish_user(&existing.shard, &user_uuid, event)
            .await?;
        await_seq(&self.users_applied, seq).await;
        Ok(seq)
    }

    pub async fn change_password(&self, uid: &Uuid, current: &str, new: &str) -> MyResult<u64> {
        let existing = self
            .user_repository
            .find_auth_by_id(uid)
            .await?
            .context_not_found(("Not Found", "Could not find user"))?;

        if !password::verify(&existing.password, current) {
            return Err(MyError::unauthorized("Unauthorized", "Incorrect password"));
        }

        let user_uuid = existing.uid;
        let event = UserEvent::PasswordChanged(UserPasswordChanged {
            user_id: user_uuid,
            password_hash: password::hash(new)?,
        });

        let seq = self
            .publish_user(&existing.shard, &user_uuid, event)
            .await?;
        await_seq(&self.users_applied, seq).await;
        Ok(seq)
    }

    async fn publish_user(&self, shard: &str, user_uuid: &Uuid, event: UserEvent) -> MyResult<u64> {
        bus::publish(
            &self.js,
            user_subject(shard, user_uuid),
            &Envelope::new(event, Some(*user_uuid)),
        )
        .await
    }

    /// Returns `(jwt, refresh_expiry, jti)`.
    fn mint_jwt(&self, user_uuid: &Uuid) -> MyResult<(String, DateTime<Utc>, Uuid)> {
        let now = Utc::now();
        let jti = Uuid::new_v4();
        let jwt = generate_jwt(
            user_uuid,
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
            &Envelope::new(event, Some(*user_uuid)),
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
