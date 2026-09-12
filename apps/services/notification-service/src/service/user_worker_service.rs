//! Everything `worker::users::UserWorker` does with a USERS event.
//!
//! Named for its caller, which is a `Worker` and never a route — this service has no
//! routes at all beyond health. The worker above decodes and acks; the decision of
//! *which* mail an event deserves, and the minting of anything that goes into it, is
//! here.
//!
//! The extension point for the whole service is the `match` in [`Self::mail_for`]: a
//! new email is a new arm returning a new [`Mail`] variant. Events that mean nothing to
//! a mailbox return `None` and are acked without a send.

use std::sync::Arc;

use shared::{
    email_token::{self, Purpose},
    error::myerror::MyResult,
    events::{Envelope, user::UserEvent},
    notification::Mail,
};
use uuid::Uuid;

use crate::{CONFIG, client::mailer::Mailer};

pub struct UserWorkerService {
    /// Kept as [`Mailer`] rather than folded in here: that type is the only place
    /// `resend_rs` is touched, and the seam is worth naming. See `client/mailer.rs`.
    pub mailer: Arc<Mailer>,
}

impl UserWorkerService {
    pub fn new(mailer: Arc<Mailer>) -> Self {
        Self { mailer }
    }

    /// Sends whatever this event deserves, keyed on the event id so a redelivery is
    /// recognised by the provider rather than sent twice.
    pub async fn notify(&self, envelope: &Envelope<UserEvent>) -> MyResult<()> {
        // The one place in the codebase that reads this flag, and the reason it
        // exists. A backfill re-emits `Registered` for every account so a projection
        // can be rebuilt from current state — harmless for a database, and a fresh
        // verification email to every user on the system if it reached here.
        //
        // Guarded here rather than in the worker so that anything else this service
        // grows is covered by the same check: sending mail is what must not repeat,
        // not consuming the event.
        if envelope.backfill {
            return Ok(());
        }

        let Some(mail) = self.mail_for(&envelope.payload)? else {
            return Ok(());
        };

        self.mailer.send(&mail, &envelope.event_id).await
    }

    fn mail_for(&self, event: &UserEvent) -> MyResult<Option<Mail>> {
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
/// event.
///
/// The second half used to read "`STREAM_USERS` has no `max_age`; anything
/// published there is a permanent record". It expires after a week now, but the
/// reasoning survives the change intact: a week is still far longer than the
/// token's 24 hours, and a bearer credential has no business in a log that anything
/// downstream can replay at all.
fn verification_url(user_id: &Uuid) -> MyResult<String> {
    let token = email_token::mint(
        &CONFIG.email_token_secret,
        user_id,
        Purpose::VerifyEmail,
        CONFIG.verify_token_ttl_secs,
    )?;
    Ok(format!("{}/verify?token={token}", CONFIG.app_base_url))
}
