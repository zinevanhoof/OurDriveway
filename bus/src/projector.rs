use std::{sync::Arc, time::Duration};

use async_nats::jetstream::{Context, consumer::DeliverPolicy};
use chrono::{DateTime, Utc};
use futures::StreamExt;
use shared::{
    db::Cursor,
    error::myerror::{MyError, MyResult},
    events::Envelope,
};
use surrealdb::{Surreal, engine::remote::ws::Client, method::Transaction};

use crate::health::Readiness;

/// Applies a stream's events to this instance's own database.
///
/// Implementations must be **deterministic**: no `time::now()`, no `rand`, no
/// database-generated ids. Every replica replays the same events independently, so
/// anything derived from a local clock or a random source makes them disagree
/// permanently. Timestamps and ids come from the event.
///
/// The two usual ways to break that are gone by construction here: there is no cursor
/// to forget — [`Tx`] advances it in the same transaction as the data — and no clock to
/// reach for other than the `at` handed in. Multi-statement applies are genuinely atomic
/// for the same reason, rather than a hand-rolled `BEGIN … COMMIT` in one query string.
///
/// Wrap one in a [`Tx`] to run it: `bus::projector::run(js, Tx::new(MyProjector, db),
/// readiness)`.
pub trait Projector: Send + Sync + 'static {
    const STREAM: &'static str;

    /// The event enum carried by this stream's envelopes.
    type Event: serde::de::DeserializeOwned + Send;

    /// Apply one decoded event inside an open transaction. Must be idempotent.
    ///
    /// `at` is the envelope's `occurred_at` — the only clock an implementation may
    /// use, because every replica has to derive the same timestamps from the same
    /// event.
    ///
    /// `seq` is this event's position in the stream. Most projectors ignore it —
    /// advancing the cursor is [`Tx`]'s job and deliberately not reachable from
    /// here. It is passed because a projection may legitimately *store* a stream
    /// position as data: booking-service keeps a per-spot `bookings_seq` that
    /// reserve asserts as a compare-and-swap precondition, and it has to be the
    /// same number the cursor moves to.
    fn apply(
        &self,
        tx: &Transaction<Client>,
        event: Self::Event,
        at: DateTime<Utc>,
        seq: u64,
    ) -> impl Future<Output = MyResult<()>> + Send;
}

/// Drives a [`Projector`], owning the three things the inner type is then unable to get
/// wrong: decoding the envelope, opening and closing the transaction, and advancing the
/// cursor inside it.
///
/// This is what [`run`] consumes — a bare `Projector` cannot be run, because none of the
/// above would happen.
pub struct Tx<P> {
    projector: P,
    /// The one client this projector's transactions are opened on, parked here
    /// between events.
    ///
    /// `Surreal::begin` consumes the client and `commit`/`cancel` hand it back, so
    /// cycling it through here means the projector never clones. That is not a
    /// micro-optimisation: `Surreal::clone` mints a *session*, and the WS engine
    /// replays every `replayable()` command onto it — for this codebase `Attach`,
    /// `Signin` and `Use`. A clone per event re-authenticates with root over the
    /// socket before every applied event, which a cold replay pays for once per
    /// message in the stream.
    ///
    /// No lock: [`run`] owns this and applies events one at a time, so `&mut self`
    /// is the whole of the mutual exclusion. `Option` only so the client can be
    /// moved into `begin` and back — `None` mid-event, and after a failed commit
    /// or rollback, which stops the projector for good so nothing observes it.
    client: Option<Surreal<Client>>,
}

impl<P> Tx<P> {
    /// Takes the client by value rather than borrowing one from `projector`: this
    /// is the only place a transaction can be opened, so this is the only thing
    /// that needs a connection. A `Projector` therefore holds none at all and
    /// cannot reach the database except through the `&Transaction` it is handed.
    pub fn new(projector: P, client: Surreal<Client>) -> Self {
        Self {
            projector,
            client: Some(client),
        }
    }

    fn client(&self) -> MyResult<&Surreal<Client>> {
        self.client
            .as_ref()
            .ok_or_else(|| bus_err("projector has no connection; it already failed".into()))
    }
}

impl<P: Projector> Tx<P> {
    /// Highest stream sequence already applied. 0 when the projection is empty.
    async fn last_seq(&self) -> MyResult<u64> {
        Cursor::last_seq(self.client()?, P::STREAM).await
    }

    /// Decode, apply and bump the cursor, all inside one transaction.
    ///
    /// `&mut self` because [`run`] owns this and applies events strictly one at a time.
    /// That is the whole of the mutual exclusion protecting `client`.
    async fn apply(&mut self, payload: &[u8], seq: u64) -> MyResult<()> {
        let envelope: Envelope<P::Event> = serde_json::from_slice(payload).map_err(|e| {
            shared::error::myerror::MyError::Bus(format!("decode {} at seq {seq}: {e}", P::STREAM))
        })?;
        let at = envelope.occurred_at;
        let cursor = Cursor {
            stream: P::STREAM,
            at,
            seq,
        };

        self.client()?;
        let client = self.client.take().expect("checked");

        let tx = client.begin().await?;

        // One `Result` for both statements, so a failing cursor bump takes the
        // same cancel path as a failing apply. An early `?` here would drop the
        // transaction instead: `Transaction` is #[must_use] and holds state on the
        // server, so a dropped one keeps whatever it locked until the server times
        // it out.
        let applied = async {
            self.projector.apply(&tx, envelope.payload, at, seq).await?;
            cursor.bump(&tx).await
        }
        .await;

        match applied {
            Ok(()) => {
                self.client = Some(tx.commit().await?);
                Ok(())
            }
            Err(e) => {
                // Best effort. If the rollback itself fails the client is gone
                // with it, but so is this projector: the error returned here stops
                // the loop, so no later event looks for it.
                if let Ok(client) = tx.cancel().await {
                    self.client = Some(client);
                }
                Err(e)
            }
        }
    }
}

/// Runs a projector until it fails. Spawn one per stream.
///
/// On an apply error this **stops** rather than skipping the message. A skipped
/// event makes this instance permanently disagree with its peers, which is far
/// worse than being drained: `/readyz` goes 503 and the load balancer takes it
/// out of rotation, leaving a diagnosable stopped replica instead of a silently
/// wrong one.
/// Takes the [`Tx`] by value: this is its sole owner, which is what makes
/// `apply(&mut self)` and the state it protects possible.
pub async fn run<P: Projector>(js: Context, projector: Tx<P>, readiness: Arc<Readiness>) {
    if let Err(e) = run_inner(js, projector, &readiness).await {
        tracing::error!(stream = P::STREAM, error = %e, "projector stopped");
        readiness.mark_failed(P::STREAM);
    }
}

async fn run_inner<P: Projector>(
    js: Context,
    mut projector: Tx<P>,
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

fn bus_err(msg: String) -> MyError {
    MyError::Bus(msg)
}
