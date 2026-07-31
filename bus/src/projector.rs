use std::{sync::Arc, time::Duration};

use async_nats::jetstream::{Context, consumer::DeliverPolicy};
use futures::StreamExt;
use shared::error::myerror::MyResult;

use crate::health::Readiness;

/// Applies a stream's events to this instance's own database.
///
/// Implementations must be **deterministic**: no `time::now()`, no `rand`, no
/// database-generated ids. Every replica replays the same events independently,
/// so anything derived from a local clock or a random source makes them disagree
/// permanently. Timestamps and ids come from the event.
///
/// `apply` must also persist the cursor **in the same transaction** as the data.
/// Split them and a crash between the two either replays applied events (harmless
/// if `apply` is idempotent) or skips unapplied ones (silent corruption).
pub trait Projector: Send + Sync + 'static {
    const STREAM: &'static str;

    /// Highest stream sequence already applied. 0 when the projection is empty.
    fn last_seq(&self) -> impl Future<Output = MyResult<u64>> + Send;

    /// Apply one event. Must be idempotent — at-least-once delivery means this
    /// will be handed the same message twice eventually.
    fn apply(&self, payload: &[u8], seq: u64) -> impl Future<Output = MyResult<()>> + Send;
}

/// Runs a projector until it fails. Spawn one per stream.
///
/// On an apply error this **stops** rather than skipping the message. A skipped
/// event makes this instance permanently disagree with its peers, which is far
/// worse than being drained: `/readyz` goes 503 and the load balancer takes it
/// out of rotation, leaving a diagnosable stopped replica instead of a silently
/// wrong one.
pub async fn run<P: Projector>(js: Context, projector: Arc<P>, readiness: Arc<Readiness>) {
    if let Err(e) = run_inner(js, projector, &readiness).await {
        tracing::error!(stream = P::STREAM, error = %e, "projector stopped");
        readiness.mark_failed(P::STREAM);
    }
}

async fn run_inner<P: Projector>(
    js: Context,
    projector: Arc<P>,
    readiness: &Arc<Readiness>,
) -> MyResult<()> {
    let cursor = projector.last_seq().await?;
    let mut stream = js
        .get_stream(P::STREAM)
        .await
        .map_err(|e| bus_err(format!("get stream {}: {e}", P::STREAM)))?;

    // Snapshot the head *before* consuming: that is the point at which this
    // instance can be considered current. Anything appended after is normal
    // steady-state lag, not a cold start.
    let target = stream
        .info()
        .await
        .map_err(|e| bus_err(format!("stream info: {e}")))?
        .state
        .last_sequence;

    tracing::info!(stream = P::STREAM, cursor, target, "replaying");
    let mut caught_up = cursor >= target;
    if caught_up {
        readiness.mark_caught_up(P::STREAM);
    }

    let mut consumer = stream
        .create_consumer(async_nats::jetstream::consumer::pull::Config {
            // Ephemeral: every instance consumes every message (this is fan-out,
            // not work-sharing), and the resume cursor lives in the projection
            // database — the only place it can be transactional with the data.
            deliver_policy: DeliverPolicy::ByStartSequence {
                start_sequence: cursor + 1,
            },
            ..Default::default()
        })
        .await
        .map_err(|e| bus_err(format!("create consumer: {e}")))?;

    let mut messages = consumer
        .messages()
        .await
        .map_err(|e| bus_err(format!("consume: {e}")))?;

    loop {
        // While catching up, wait only briefly, so an *empty* stream can still
        // settle: `target` is the head sequence at boot, but that message may
        // since have been deleted, in which case `seq >= target` never fires and
        // the instance would sit at 503 forever waiting for something
        // unreachable. Asking the consumer what it still has pending is the
        // reliable answer.
        //
        // Once caught up, block outright. The timeout is not a rate limit — it
        // resolves the moment a message arrives — but constructing one per
        // iteration registers a timer per event, and firing it forever on an idle
        // stream is two pointless wakeups a second, per projector, per instance.
        let next = if caught_up {
            Ok(messages.next().await)
        } else {
            tokio::time::timeout(Duration::from_millis(500), messages.next()).await
        };

        let msg = match next {
            Ok(Some(msg)) => msg.map_err(|e| bus_err(format!("next message: {e}")))?,
            Ok(None) => return Err(bus_err("message stream ended".to_string())),
            Err(_idle) => {
                if !caught_up {
                    let pending = consumer
                        .info()
                        .await
                        .map_err(|e| bus_err(format!("consumer info: {e}")))?
                        .num_pending;
                    if pending == 0 {
                        caught_up = true;
                        readiness.mark_caught_up(P::STREAM);
                    }
                }
                continue;
            }
        };

        let seq = msg
            .info()
            .map_err(|e| bus_err(format!("message info: {e}")))?
            .stream_sequence;

        projector.apply(&msg.payload, seq).await?;

        // Only after the transaction committed: this is what request handlers
        // block on for read-your-own-writes.
        readiness.set_applied(P::STREAM, seq);
        msg.ack()
            .await
            .map_err(|e| bus_err(format!("ack {seq}: {e}")))?;

        if !caught_up && seq >= target {
            caught_up = true;
            readiness.mark_caught_up(P::STREAM);
        }
    }
}

fn bus_err(msg: String) -> shared::error::myerror::MyError {
    shared::error::myerror::MyError::Bus(msg)
}
