use bus::Projector;
use chrono::{DateTime, Utc};
use shared::{
    domain_models::user::{RefreshToken, RefreshTokenPatch, UserPatch},
    error::myerror::MyResult,
    events::{
        STREAM_SESSIONS, STREAM_USERS,
        session::SessionEvent,
        user::{UserEvent, UserUpdated},
    },
};
use surrealdb::{engine::remote::ws::Client, method::Transaction};

use crate::repository::{
    refresh_token_repository::RefreshTokenRepository, user_repository::UserRepository,
};

/// Applies USERS events to this instance's local projection — the only writer to
/// the `user` table. Handlers publish; they never write.
///
/// Holds no connection, deliberately: `bus::Tx` owns the only one and hands this
/// a `&Transaction` per event, so there is no path from here to the database that
/// bypasses the transaction. The repository is built over that handle, so every
/// statement below joins it.
pub struct UserProjector;

impl Projector for UserProjector {
    const STREAM: &'static str = STREAM_USERS;
    type Event = UserEvent;

    async fn apply(
        &self,
        tx: &Transaction<Client>,
        event: UserEvent,
        _at: DateTime<Utc>,
        _seq: u64,
    ) -> MyResult<()> {
        let users = UserRepository { q: tx };

        match event {
            // UPSERT keyed by the event's own id, not CREATE: replay must be
            // idempotent, and a database-generated id would differ per replica.
            UserEvent::Registered(e) => users.upsert(e.into()).await,

            UserEvent::Updated(e) => Self::updated(&users, e).await,

            UserEvent::PasswordChanged(e) => {
                users
                    .patch(
                        e.user_id,
                        UserPatch {
                            password: Some(e.password_hash),
                            ..Default::default()
                        },
                    )
                    .await
            }

            // Idempotent by construction — setting `true` twice is setting `true`.
            // That matters because mail scanners prefetch links, so the endpoint
            // that publishes this is deliberately re-runnable.
            UserEvent::EmailVerified { user_id } => {
                users
                    .patch(
                        user_id,
                        UserPatch {
                            email_verified: Some(true),
                            ..Default::default()
                        },
                    )
                    .await
            }

            // Purely a message to notification-service; nothing here changes. The
            // cursor still has to move, which `Tx` does after this returns —
            // otherwise a restart replays from before it, forever.
            UserEvent::VerificationRequested(_) => Ok(()),
        }
    }
}

impl UserProjector {
    /// The one arm that is more than a single statement, and the order is the
    /// point: the submitted address has to be compared against the stored one
    /// *before* it is overwritten.
    ///
    /// Without this, changing to an unverified address keeps the flag from the old
    /// one and login lets it straight through — which makes the whole feature
    /// decorative. Both statements run in the caller's transaction, so a crash
    /// between them cannot leave the address changed with the flag still set.
    async fn updated(users: &UserRepository<&Transaction<Client>>, e: UserUpdated) -> MyResult<()> {
        let user_id = e.user_id;
        let current = users.find_by_id(user_id).await?;

        if let Some(current) = current
            && e.email.as_ref().is_some_and(|new| *new != current.email)
        {
            users
                .patch(
                    user_id,
                    UserPatch {
                        email_verified: Some(false),
                        ..Default::default()
                    },
                )
                .await?;
        }

        users.patch(user_id, e.into()).await
    }
}

/// Applies SESSIONS events to the local `refresh_token` projection.
///
/// Separate consumer from USERS, so a login can in principle be applied before the
/// account it belongs to — harmless here, because a refresh token is looked up by
/// its own hash and never traverses to the user row.
pub struct SessionProjector;

impl Projector for SessionProjector {
    const STREAM: &'static str = STREAM_SESSIONS;
    type Event = SessionEvent;

    async fn apply(
        &self,
        tx: &Transaction<Client>,
        event: SessionEvent,
        at: DateTime<Utc>,
        _seq: u64,
    ) -> MyResult<()> {
        let tokens = RefreshTokenRepository { q: tx };

        match event {
            SessionEvent::Issued(e) => tokens.upsert(RefreshToken::issued(e, at)).await,

            // Revoke-and-issue, mirroring the single event. Two statements, one
            // transaction: there is no instant at which the old token is dead and
            // the new one does not exist.
            SessionEvent::Rotated(e) => {
                tokens
                    .patch_by_token_hash(
                        e.old_token_hash.clone(),
                        RefreshTokenPatch {
                            revoked: Some(true),
                            revoked_reason: Some("Rotation".to_string()),
                            ..Default::default()
                        },
                    )
                    .await?;
                tokens.upsert(RefreshToken::rotated(e, at)).await
            }

            SessionEvent::Revoked(e) => {
                tokens
                    .patch_by_token_hash(
                        e.token_hash,
                        RefreshTokenPatch {
                            revoked: Some(true),
                            revoked_reason: Some(e.reason),
                            ..Default::default()
                        },
                    )
                    .await?;
                // `at` is the event's own clock, so every replica drops exactly
                // the same rows.
                tokens.delete_expired(at).await
            }
        }
    }
}
