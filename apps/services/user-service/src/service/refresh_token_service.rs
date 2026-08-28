use chrono::Utc;
use shared::domain_models::user::{RefreshToken, RefreshTokenPatch, User};
use shared::error::myerror::{ContextExt, MyResult};
use shared::events::session::{
    RefreshTokenIssued, RefreshTokenRevoked, RefreshTokenRotated, SessionEvent,
};
use bus::outbox;
use shared::db;
use shared::events::{Envelope, aggregate_id, format_version, session_subject};
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
/// Nothing here waits on a projection either — see
/// [`crate::service::user_service::UserService`].
///
/// This one used to be the strongest case for it: the refresh token goes into an
/// httponly cookie, so the *token* is not something the client can echo. But the
/// SESSIONS position is, and `AuthResponse` now carries it, so `/refresh` reading
/// the projection too early is covered by the layer on whichever replica takes it —
/// which the old per-instance wait never was.
pub struct RefreshTokenService {
    pub tokens: RefreshTokenRepository,
}

impl RefreshTokenService {
    /// A new session for an already-authenticated user.
    ///
    /// Takes the `User` rather than an id: the caller has just read the row to
    /// check the password. Sessions publish onto the *user's* subject, so one
    /// user's whole session history stays on one ordered subject.
    ///
    /// Returns `(access token, refresh token, await token)`. The last is
    /// `refresh_token:<id>@<version>`, which the client echoes so its next
    /// `/refresh` sees this session.
    pub async fn issue(&self, user: &User) -> MyResult<(String, String, String)> {
        let refresh_token = Uuid::new_v4();
        let (jwt, expires_at, jti) = jwt::mint(&user.id)?;
        let token_id = Uuid::now_v7();

        // The session is its own aggregate, keyed by the token rather than by the
        // user: two logins from two devices are two independent rows, and pinning
        // them to a shared user version would make either one conflict with an
        // unrelated profile edit.
        let issued = RefreshTokenIssued {
            token_id,
            user_id: user.id,
            // Only the hash travels; the plaintext goes to the client's cookie and
            // nowhere else.
            token_hash: token::hash(&refresh_token),
            jti,
            expires_at,
        };

        let tx = db::begin(&self.tokens.q).await?;
        let version = db::next_version(&tx, "refresh_token", &token_id).await?;

        RefreshTokenRepository { q: &tx }
            .upsert(RefreshToken::issued(issued.clone(), Utc::now()))
            .await?;
        db::set_version(&tx, "refresh_token", &token_id, version).await?;

        let envelope = Envelope::new(
            SessionEvent::Issued(issued),
            Some(user.id),
            aggregate_id("refresh_token", &token_id),
            version,
        );
        let await_token = format_version(&envelope.aggregate, envelope.version);

        // Still the user's subject: one user's whole session history stays on one
        // ordered subject, even though each session versions independently.
        outbox::enqueue(&tx, &session_subject(&user.id), &envelope).await?;
        tx.commit().await?;

        Ok((jwt, refresh_token.to_string(), await_token))
    }

    /// Trades a valid refresh token for a fresh pair.
    pub async fn rotate(&self, old_refresh_token: Uuid) -> MyResult<(String, String, String)> {
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
        // Versioned against the NEW token: rotation mints a fresh row, and that is
        // the one a following `/refresh` has to be able to see.
        let token_id = Uuid::now_v7();
        let rotated = RefreshTokenRotated {
            old_token_hash: old_hash,
            token_id,
            user_id: user_uuid,
            token_hash: token::hash(&new_refresh),
            jti,
            expires_at,
        };

        let tx = db::begin(&self.tokens.q).await?;
        let tokens = RefreshTokenRepository { q: &tx };
        let version = db::next_version(&tx, "refresh_token", &token_id).await?;

        // Revoke-and-issue in one transaction, mirroring the single event: there is
        // no instant at which the old token is dead and the new one does not exist.
        tokens
            .patch_by_token_hash(
                rotated.old_token_hash.clone(),
                RefreshTokenPatch {
                    revoked: Some(true),
                    revoked_reason: Some("Rotation".to_string()),
                    ..Default::default()
                },
            )
            .await?;
        tokens
            .upsert(RefreshToken::rotated(rotated.clone(), Utc::now()))
            .await?;
        db::set_version(&tx, "refresh_token", &token_id, version).await?;

        let envelope = Envelope::new(
            SessionEvent::Rotated(rotated),
            Some(user_uuid),
            aggregate_id("refresh_token", &token_id),
            version,
        );
        let await_token = format_version(&envelope.aggregate, envelope.version);

        outbox::enqueue(&tx, &session_subject(&user_uuid), &envelope).await?;
        tx.commit().await?;

        Ok((jwt, new_refresh.to_string(), await_token))
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
        let revoked = RefreshTokenRevoked {
            token_hash: hash,
            reason: "logout".to_string(),
        };

        let tx = db::begin(&self.tokens.q).await?;
        let tokens = RefreshTokenRepository { q: &tx };
        let version = db::next_version(&tx, "refresh_token", &existing.id).await?;

        tokens
            .patch_by_token_hash(
                revoked.token_hash.clone(),
                RefreshTokenPatch {
                    revoked: Some(true),
                    revoked_reason: Some(revoked.reason.clone()),
                    ..Default::default()
                },
            )
            .await?;
        // `Utc::now()` is this process's clock, which is fine now that this is the
        // only writer — it used to have to be the envelope's, because every replica
        // replayed the same event and had to drop exactly the same rows.
        tokens.delete_expired(Utc::now()).await?;
        db::set_version(&tx, "refresh_token", &existing.id, version).await?;

        let envelope = Envelope::new(
            SessionEvent::Revoked(revoked),
            Some(existing.user_id),
            aggregate_id("refresh_token", &existing.id),
            version,
        );

        outbox::enqueue(&tx, &session_subject(&existing.user_id), &envelope).await?;
        tx.commit().await?;

        Ok(())
    }
}
