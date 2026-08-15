use resend_rs::{Resend, types::CreateEmailBaseOptions};
use shared::{
    error::myerror::{MyError, MyResult},
    notification::Mail,
};
use uuid::Uuid;

use crate::{CONFIG, template};

/// The only place `resend_rs` is touched.
///
/// Everything above this speaks in [`Mail`], so swapping provider — or dropping
/// to raw SMTP the day a calendar invite needs a real `text/calendar` part —
/// changes this file and nothing else.
pub struct Mailer {
    client: Resend,
    /// `NOTIFICATIONS_ENABLED`. False makes [`Self::send`] build the mail and drop
    /// it — see the note there for why the gate is at this depth and not higher.
    enabled: bool,
}

impl Mailer {
    pub fn new(api_key: &str, enabled: bool) -> Self {
        Self {
            client: Resend::new(api_key),
            enabled,
        }
    }

    /// Sends one mail, keyed on the event that caused it.
    ///
    /// `event_id` becomes the `Idempotency-Key`. It is what makes the whole
    /// service safe under at-least-once delivery: a crash between the provider
    /// accepting the message and the NATS ack redelivers the event, and Resend
    /// recognises the repeat and does not send twice. That is the entire reason
    /// this service needs no database.
    ///
    /// With `NOTIFICATIONS_ENABLED=false` everything here runs except the send
    /// itself, and the caller gets `Ok(())` — so the worker acks and its durable
    /// consumer keeps moving.
    ///
    /// The gate is HERE rather than around the worker in `main`, for two reasons.
    /// A worker that is never spawned stops acking, so `notification-users` piles
    /// up a backlog and re-enabling mails everyone who registered in the meantime —
    /// the exact stampede the `bus::worker` comment above the spawn exists to
    /// prevent. And building the mail first means the template and token paths
    /// still execute, so a broken template fails in dev instead of hiding until
    /// the day this is switched back on.
    pub async fn send(&self, mail: &Mail, event_id: &Uuid) -> MyResult<()> {
        let email = CreateEmailBaseOptions::new(
            CONFIG.mail_from.as_str(),
            [mail.to()],
            template::subject(mail),
        )
        .with_template(
            resend_rs::types::EmailTemplate::new(template::template_id(mail))
                .with_variables(template::variables(mail)),
        )
        .with_idempotency_key(&event_id.to_string());

        if !self.enabled {
            // No recipient in this line, same rule as the error arm below.
            tracing::info!(
                kind = mail.kind(),
                %event_id,
                "NOTIFICATIONS_ENABLED=false; mail built and dropped"
            );
            return Ok(());
        }

        let sent = self.client.emails.send(email).await.map_err(|e| {
            // The address is deliberately absent: this line goes to stdout, gets
            // scraped, and a bounce message is not worth putting a user's email
            // into a log aggregator.
            MyError::Bus(format!("resend send {}: {e}", mail.kind()))
        })?;

        tracing::info!(kind = mail.kind(), id = %sent.id, "sent");
        Ok(())
    }
}
