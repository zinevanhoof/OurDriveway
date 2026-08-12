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
}

impl Mailer {
    pub fn new(api_key: &str) -> Self {
        Self {
            client: Resend::new(api_key),
        }
    }

    /// Sends one mail, keyed on the event that caused it.
    ///
    /// `event_id` becomes the `Idempotency-Key`. It is what makes the whole
    /// service safe under at-least-once delivery: a crash between the provider
    /// accepting the message and the NATS ack redelivers the event, and Resend
    /// recognises the repeat and does not send twice. That is the entire reason
    /// this service needs no database.
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
