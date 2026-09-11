use bus::outbox;
use chrono::Utc;
use diesel_async::AsyncConnection;
use diesel_async::scoped_futures::ScopedFutureExt;
use shared::db;
use shared::domain_models::user::{RefreshToken, RefreshTokenPatch, User};
use shared::error::myerror::{ContextExt, MyError, MyResult};
use shared::events::session::{
    RefreshTokenIssued, RefreshTokenRevoked, RefreshTokenRotated, SessionEvent,
};
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
/// SESSIONS position is, and login's `X-Version` header now carries it, so `/refresh` reading
/// the projection too early is covered by the layer on whichever replica takes it —
/// which the old per-instance wait never was.
pub struct RefreshTokenService {
    /// The pool. See the note on `UserService::db` — the repositories are stateless.
    pub db: shared::db::Db,
}

impl RefreshTokenService {
    /// A new session for an already-authenticated user.
    ///
    /// Takes the `User` rather than an id: the caller has just read the row to
    /// check the password. Sessions publish onto the *user's* subject, so one
    /// user's whole session history stays on one ordered subject.
    ///
    /// Returns `(access token, refresh token, version)`. The last is
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

        let mut conn = db::conn(&self.db).await?;
        let user_id = user.id;

        // `transaction` owns the begin, the commit and the rollback: `Ok` commits, `Err`
        // rolls back, and there is no path that can forget either. `scope_boxed` is
        // required by the signature — it cannot be generic over an arbitrary future
        // without boxing (rustc#100013).
        let version = conn
            .transaction::<_, MyError, _>(|conn| {
                async move {
                    let version = shared::next_version!(conn, shared::schema::user::refresh_token, &token_id)?;

                    RefreshTokenRepository::upsert(
                        conn,
                        RefreshToken::issued(issued.clone(), Utc::now()),
                    )
                    .await?;
                    shared::set_version!(conn, "refresh_token", shared::schema::user::refresh_token, &token_id, version)?;

                    let envelope = Envelope::new(
                        SessionEvent::Issued(issued),
                        Some(user_id),
                        aggregate_id("refresh_token", &token_id),
                        version,
                    );

                    // Still the user's subject: one user's whole session history stays on
                    // one ordered subject, even though each session versions
                    // independently.
                    outbox::enqueue(conn, &session_subject(&user_id), &envelope).await?;
                    Ok(format_version(&envelope.aggregate, envelope.version))
                }
                .scope_boxed()
            })
            .await?;

        Ok((jwt, refresh_token.to_string(), version))
    }

    /// Trades a valid refresh token for a fresh pair.
    pub async fn rotate(&self, old_refresh_token: Uuid) -> MyResult<(String, String, String)> {
        let old_hash = token::hash(&old_refresh_token);
        // Unknown, revoked and expired all collapse into the same 401. They used to
        // differ — a 404 for unknown, a 401 for the rest — which told an
        // unauthenticated caller which token values had once existed. Same reason
        // `revoke` shrugs at a token it cannot find.
        // Bound, then reborrowed: `PooledConnection` derefs to `AsyncPgConnection`, but a
        // temporary in argument position is not coerced through it.
        let mut read = db::conn(&self.db).await?;
        let existing = RefreshTokenRepository::find_by_token_hash(&mut read, old_hash.clone())
            .await?
            // `expires_at` is a plain `DateTime<Utc>` now, so no `into_inner()` to
            // unwrap a driver newtype first.
            .filter(|t| !t.revoked && t.expires_at >= Utc::now())
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

        let mut conn = db::conn(&self.db).await?;

        let version = conn
            .transaction::<_, MyError, _>(|conn| {
                async move {
                    let version = shared::next_version!(conn, shared::schema::user::refresh_token, &token_id)?;

                    // Revoke-and-issue in one transaction, mirroring the single event:
                    // there is no instant at which the old token is dead and the new one
                    // does not exist.
                    RefreshTokenRepository::patch_by_token_hash(
                        conn,
                        rotated.old_token_hash.clone(),
                        RefreshTokenPatch {
                            revoked: Some(true),
                            revoked_reason: Some("Rotation".to_string()),
                            ..Default::default()
                        },
                    )
                    .await?;
                    RefreshTokenRepository::upsert(
                        conn,
                        RefreshToken::rotated(rotated.clone(), Utc::now()),
                    )
                    .await?;
                    shared::set_version!(conn, "refresh_token", shared::schema::user::refresh_token, &token_id, version)?;

                    let envelope = Envelope::new(
                        SessionEvent::Rotated(rotated),
                        Some(user_uuid),
                        aggregate_id("refresh_token", &token_id),
                        version,
                    );

                    outbox::enqueue(conn, &session_subject(&user_uuid), &envelope).await?;
                    Ok(format_version(&envelope.aggregate, envelope.version))
                }
                .scope_boxed()
            })
            .await?;

        Ok((jwt, new_refresh.to_string(), version))
    }

    /// Ends a session. An unknown token is not an error: logging out something
    /// already gone is the desired end state, and saying otherwise would confirm
    /// which tokens exist.
    pub async fn revoke(&self, refresh_token: Uuid) -> MyResult<()> {
        let hash = token::hash(&refresh_token);
        let mut read = db::conn(&self.db).await?;
        let Some(existing) =
            RefreshTokenRepository::find_by_token_hash(&mut read, hash.clone()).await?
        else {
            return Ok(());
        };

        // Nothing waits on a logout: the session it ends is the one the caller is
        // giving up, and the cookie is cleared in the same response.
        let revoked = RefreshTokenRevoked {
            token_hash: hash,
            reason: "logout".to_string(),
        };

        let mut conn = db::conn(&self.db).await?;

        conn.transaction::<_, MyError, _>(|conn| {
            async move {
                let version = shared::next_version!(conn, shared::schema::user::refresh_token, &existing.id)?;

                RefreshTokenRepository::patch_by_token_hash(
                    conn,
                    revoked.token_hash.clone(),
                    RefreshTokenPatch {
                        revoked: Some(true),
                        revoked_reason: Some(revoked.reason.clone()),
                        ..Default::default()
                    },
                )
                .await?;
                // `Utc::now()` is this process's clock, which is fine now that this is
                // the only writer — it used to have to be the envelope's, because every
                // replica replayed the same event and had to drop exactly the same rows.
                RefreshTokenRepository::delete_expired(conn, Utc::now()).await?;
                shared::set_version!(conn, "refresh_token", shared::schema::user::refresh_token, &existing.id, version)?;

                let envelope = Envelope::new(
                    SessionEvent::Revoked(revoked),
                    Some(existing.user_id),
                    aggregate_id("refresh_token", &existing.id),
                    version,
                );

                outbox::enqueue(conn, &session_subject(&existing.user_id), &envelope).await?;
                Ok(())
            }
            .scope_boxed()
        })
        .await?;

        Ok(())
    }
}
