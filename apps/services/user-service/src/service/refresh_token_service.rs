use async_nats::jetstream::Context;
use chrono::Utc;
use shared::domain_models::user::User;
use shared::error::myerror::{ContextExt, MyResult};
use shared::events::session::{
    RefreshTokenIssued, RefreshTokenRevoked, RefreshTokenRotated, SessionEvent,
};
use shared::events::{Envelope, session_subject};
use uuid::Uuid;

use crate::{
    auth::{jwt, token},
    repository::refresh_token_repository::RefreshTokenRepository,
};

/// Sessions: issuing, rotating and revoking refresh tokens.
///
/// Split from [`crate::service::user_service::UserService`] because the two answer
/// different questions — that one owns who a user *is*, this one owns whether a
/// caller is still logged in — and they share no state beyond the connection.
/// Neither calls the other; `route::login` calls both in turn.
/// No `await_applied` here either — see [`crate::service::user_service::UserService`].
///
/// This one used to be the strongest case for it: the refresh token goes into an
/// httponly cookie, so the *token* is not something the client can echo. But the
/// SESSIONS position is, and `AuthResponse` now carries it, so `/refresh` reading
/// the projection too early is covered by the layer on whichever replica takes it —
/// which the old per-instance wait never was.
pub struct RefreshTokenService {
    pub tokens: RefreshTokenRepository,
    pub js: Context,
}

impl RefreshTokenService {
    /// A new session for an already-authenticated user.
    ///
    /// Takes the `User` rather than an id: the caller has just read the row to
    /// check the password, and the shard on it decides which subject this event
    /// lands on — sessions live on the user's own subject, so one user's whole
    /// session history stays on one ordered subject.
    ///
    /// Returns `(access token, refresh token, log position)`. The position is what
    /// the client echoes so its next `/refresh` sees this session.
    pub async fn issue(&self, user: &User) -> MyResult<(String, String, u64)> {
        let refresh_token = Uuid::new_v4();
        let (jwt, expires_at, jti) = jwt::mint(&user.id)?;

        let seq = bus::publish(
            &self.js,
            session_subject(&user.shard, &user.id),
            &Envelope::new(
                SessionEvent::Issued(RefreshTokenIssued {
                    token_id: Uuid::now_v7(),
                    user_id: user.id,
                    shard: user.shard.clone(),
                    // Only the hash travels; the plaintext goes to the client's cookie
                    // and nowhere else.
                    token_hash: token::hash(&refresh_token),
                    jti,
                    expires_at,
                }),
                Some(user.id),
            ),
        )
        .await?;

        Ok((jwt, refresh_token.to_string(), seq))
    }

    /// Trades a valid refresh token for a fresh pair.
    pub async fn rotate(&self, old_refresh_token: Uuid) -> MyResult<(String, String, u64)> {
        let old_hash = token::hash(&old_refresh_token);
        // Unknown, revoked and expired all collapse into the same 401. They used to
        // differ — a 404 for unknown, a 401 for the rest — which told an
        // unauthenticated caller which token values had once existed. Same reason
        // `revoke` shrugs at a token it cannot find.
        let existing = self
            .tokens
            .find_by_token_hash(old_hash.clone())
            .await?
            .filter(|t| !t.revoked && t.expires_at.into_inner() >= Utc::now())
            .context_unauthorized(("Unauthorized", "Invalid refresh token"))?;

        let user_uuid = existing.user_id;
        let new_refresh = Uuid::new_v4();
        let (jwt, expires_at, jti) = jwt::mint(&user_uuid)?;

        // One event, not a revoke followed by an issue: rotation is atomic, so
        // there is no window where the old token is dead and the new one is
        // missing.
        let seq = bus::publish(
            &self.js,
            session_subject(&existing.shard, &user_uuid),
            &Envelope::new(
                SessionEvent::Rotated(RefreshTokenRotated {
                    old_token_hash: old_hash,
                    token_id: Uuid::now_v7(),
                    user_id: user_uuid,
                    shard: existing.shard.clone(),
                    token_hash: token::hash(&new_refresh),
                    jti,
                    expires_at,
                }),
                Some(user_uuid),
            ),
        )
        .await?;

        Ok((jwt, new_refresh.to_string(), seq))
    }

    /// Ends a session. An unknown token is not an error: logging out something
    /// already gone is the desired end state, and saying otherwise would confirm
    /// which tokens exist.
    pub async fn revoke(&self, refresh_token: Uuid) -> MyResult<()> {
        let hash = token::hash(&refresh_token);
        let Some(existing) = self.tokens.find_by_token_hash(hash.clone()).await? else {
            return Ok(());
        };

        // Nothing waits on a logout: the session it ends is the one the caller is
        // giving up, and the cookie is cleared in the same response.
        bus::publish(
            &self.js,
            session_subject(&existing.shard, &existing.user_id),
            &Envelope::new(
                SessionEvent::Revoked(RefreshTokenRevoked {
                    token_hash: hash,
                    reason: "logout".to_string(),
                }),
                Some(existing.user_id),
            ),
        )
        .await?;

        Ok(())
    }
}
