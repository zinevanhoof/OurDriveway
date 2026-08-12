use std::{sync::Arc, time::Duration};

use async_nats::jetstream::{
    Context,
    consumer::{AckPolicy, DeliverPolicy},
};
use futures::StreamExt;
use shared::error::myerror::MyResult;

/// Performs a **side effect** for each event on a stream — sending mail, calling
/// a third party, anything the outside world can observe.
///
/// This is the opposite of [`crate::Projector`] in every way that matters, and
/// the two must not be confused:
///
/// | | `Projector` | `Worker` |
/// |---|---|---|
/// | consumer | ephemeral, one per instance | durable, one shared by all |
/// | delivery | fan-out — every replica sees every event | work-sharing — exactly one replica sees it |
/// | cursor | the projection database | NATS, via acks |
/// | cold start | replays from sequence 1 | starts at `New`, never replays history |
/// | on error | stops, so the instance can't disagree with peers | naks and continues |
///
/// Running side effects on a `Projector` sends N copies from N replicas, and a
/// fresh replica replays the whole log and repeats every notification ever sent.
/// That is why this exists as a separate primitive rather than a flag.
///
/// `handle` sees at-least-once delivery, so it must be idempotent or carry an
/// idempotency key the downstream provider understands.
pub trait Worker: Send + Sync + 'static {
    const STREAM: &'static str;

    /// Durable consumer name, identical across every replica — that shared name
    /// is the entire mechanism by which this becomes work-sharing instead of
    /// fan-out. Changing it creates a *new* consumer starting at `New`, silently
    /// skipping everything the old one had not yet delivered.
    const DURABLE: &'static str;

    fn handle(&self, payload: &[u8], seq: u64) -> impl Future<Output = MyResult<()>> + Send;
}

/// How long a message may be in flight before NATS assumes we died and
/// redelivers it. Must exceed the slowest `handle` — an HTTP call to a mail
/// provider — or a slow send turns into a duplicate send.
const ACK_WAIT: Duration = Duration::from_secs(30);

/// Attempts before NATS gives up on a message. This **is** the retry policy;
/// there is deliberately no retry loop inside `run`.
const MAX_DELIVER: i64 = 5;

/// Backoff before a failed message comes back. Flat rather than exponential:
/// five tries at 30s covers a provider blip, and anything longer belongs in a
/// dead-letter conversation we haven't needed yet.
const NAK_DELAY: Duration = Duration::from_secs(30);

/// Runs a worker until the connection fails. Spawn one per stream.
///
/// Unlike `projector::run` this does not touch `Readiness`. A worker builds no
/// projection, so there is nothing for it to be "caught up" with, and a service
/// that only runs workers is ready the moment it is alive.
pub async fn run<W: Worker>(js: Context, worker: Arc<W>) {
    if let Err(e) = run_inner(js, worker).await {
        tracing::error!(stream = W::STREAM, durable = W::DURABLE, error = %e, "worker stopped");
    }
}

async fn run_inner<W: Worker>(js: Context, worker: Arc<W>) -> MyResult<()> {
    let stream = js
        .get_stream(W::STREAM)
        .await
        .map_err(|e| bus_err(format!("get stream {}: {e}", W::STREAM)))?;

    let consumer = stream
        .get_or_create_consumer(W::DURABLE, consumer_config(W::DURABLE))
        .await
        .map_err(|e| bus_err(format!("create consumer {}: {e}", W::DURABLE)))?;

    let mut messages = consumer
        .messages()
        .await
        .map_err(|e| bus_err(format!("consume: {e}")))?;

    tracing::info!(stream = W::STREAM, durable = W::DURABLE, "worker running");

    while let Some(message) = messages.next().await {
        let message = message.map_err(|e| bus_err(format!("next message: {e}")))?;
        let info = message
            .info()
            .map_err(|e| bus_err(format!("message info: {e}")))?;
        let (seq, delivered) = (info.stream_sequence, info.delivered);

        match worker.handle(&message.payload, seq).await {
            Ok(()) => {
                message
                    .ack()
                    .await
                    .map_err(|e| bus_err(format!("ack {seq}: {e}")))?;
            }

            // One failure must not take the process down. A projector stops here
            // because a skipped event corrupts its database forever; a worker has
            // no database, and refusing to send the other 99 emails because one
            // address bounced is the worse outcome.
            Err(e) => {
                if delivered >= MAX_DELIVER {
                    // Last attempt — NATS will not bring this back. Logged at
                    // error so an abandoned message is visible rather than
                    // silently dropped on the floor.
                    tracing::error!(
                        stream = W::STREAM,
                        seq,
                        delivered,
                        error = %e,
                        "giving up on message after {MAX_DELIVER} attempts"
                    );
                } else {
                    tracing::warn!(stream = W::STREAM, seq, delivered, error = %e, "handler failed, will retry");
                }

                if let Err(e) = message
                    .ack_with(async_nats::jetstream::AckKind::Nak(Some(NAK_DELAY)))
                    .await
                {
                    tracing::error!(stream = W::STREAM, seq, error = %e, "nak failed");
                }
            }
        }
    }

    Err(bus_err("message stream ended".to_string()))
}

/// Split out from `run_inner` so the four settings that define this primitive can
/// be asserted without a broker. They are the entire difference between a worker
/// and a projector, and three of the four are silent when wrong: a missing
/// `durable_name` sends N copies from N replicas, and `DeliverPolicy::All`
/// replays the whole log into the outside world.
fn consumer_config(durable: &str) -> async_nats::jetstream::consumer::pull::Config {
    async_nats::jetstream::consumer::pull::Config {
        durable_name: Some(durable.to_string()),
        // Honoured only when the consumer is first created; on every later boot
        // the stored cursor wins. That is exactly the behaviour a side effect
        // wants — deploying this service must not mail every user who ever
        // registered, but a restart must not drop what arrived while it was down
        // either.
        deliver_policy: DeliverPolicy::New,
        ack_policy: AckPolicy::Explicit,
        ack_wait: ACK_WAIT,
        max_deliver: MAX_DELIVER,
        ..Default::default()
    }
}

fn bus_err(msg: String) -> shared::error::myerror::MyError {
    shared::error::myerror::MyError::Bus(msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Guards against the one mistake this module exists to prevent: someone
    /// copying `projector.rs` and getting an ephemeral fan-out consumer that
    /// mails every user from every replica.
    ///
    /// The genuine end-to-end property — two replicas, one email — needs a live
    /// broker and is checked by hand; see the verification steps in the plan.
    /// This covers the part that can regress silently in a diff.
    #[test]
    fn the_consumer_is_durable_shared_and_starts_at_the_head() {
        let config = consumer_config("notification-users");

        assert_eq!(
            config.durable_name.as_deref(),
            Some("notification-users"),
            "an ephemeral consumer makes this fan-out: every replica sends its own copy"
        );
        assert!(
            matches!(config.deliver_policy, DeliverPolicy::New),
            "any other policy replays history and re-sends every notification ever"
        );
        assert!(
            matches!(config.ack_policy, AckPolicy::Explicit),
            "without explicit acks a crash mid-send loses the message silently"
        );
        assert!(
            config.max_deliver > 1,
            "max_deliver is the retry policy; 1 means a single blip drops the mail"
        );
    }
}
