use std::sync::Arc;

use bus::Worker;
use shared::{
    email_token::{self, Purpose},
    error::myerror::{MyError, MyResult},
    events::{Envelope, STREAM_USERS, user::UserEvent},
    notification::Mail,
};
use uuid::Uuid;

use crate::{CONFIG, mailer::Mailer};

/// Turns USERS events into email.
///
/// The extension point for this whole service is the `match` in [`Self::mail_for`]:
/// a new email is a new arm returning a new [`Mail`] variant. Events that mean
/// nothing to a mailbox return `None` and are acked without a send.
pub struct UserWorker {
    pub mailer: Arc<Mailer>,
}

impl Worker for UserWorker {
    const STREAM: &'static str = STREAM_USERS;

    /// Shared by every replica. Renaming this creates a *fresh* consumer starting
    /// at `New`, silently dropping anything the old one had not delivered yet.
    const DURABLE: &'static str = "notification-users";

    async fn handle(&self, payload: &[u8], seq: u64) -> MyResult<()> {
        let envelope: Envelope<UserEvent> = serde_json::from_slice(payload)
            .map_err(|e| MyError::Bus(format!("decode UserEvent at seq {seq}: {e}")))?;

        let Some(mail) = Self::mail_for(&envelope.payload)? else {
            return Ok(());
        };

        self.mailer.send(&mail, &envelope.event_id).await
    }
}

impl UserWorker {
    fn mail_for(event: &UserEvent) -> MyResult<Option<Mail>> {
        let (user_id, email, first_name) = match event {
            UserEvent::Registered(e) => (&e.user_id, &e.email, &e.first_name),
            UserEvent::VerificationRequested(e) => (&e.user_id, &e.email, &e.first_name),

            // `Updated`, `PasswordChanged` and `EmailVerified` are nobody's
            // business here yet. Note that `Registered` also carries an Argon2
            // `password_hash` — it must never be logged, which is why nothing in
            // this file prints the event.
            _ => return Ok(None),
        };

        Ok(Some(Mail::VerifyEmail {
            to: email.clone(),
            verification_url: verification_url(user_id)?,
            first_name: Some(first_name.clone()),
            // Configured rather than hardcoded so a dev deployment can announce
            // itself as one and its mail doesn't read as production.
            company_name: Some(CONFIG.company_name.clone()),
        }))
    }
}

/// Minted here, at send time, so the 24-hour clock starts when the mail leaves
/// rather than when the account was created — and so the token never enters an
/// event. `STREAM_USERS` has no `max_age`; anything published there is a
/// permanent record.
fn verification_url(user_id: &Uuid) -> MyResult<String> {
    let token = email_token::mint(
        &CONFIG.email_token_secret,
        user_id,
        Purpose::VerifyEmail,
        CONFIG.verify_token_ttl_secs,
    )?;
    Ok(format!("{}/verify?token={token}", CONFIG.app_base_url))
}
