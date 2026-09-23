use std::{sync::Arc, time::Duration};

use async_nats::jetstream::{
    Context,
    consumer::{AckPolicy, DeliverPolicy, PullConsumer},
};
use chrono::{DateTime, Utc};
use diesel_async::scoped_futures::ScopedFutureExt;
use diesel_async::{AsyncConnection, AsyncPgConnection};
use futures::{StreamExt, TryStreamExt};
use shared::db::Db;
use shared::{
    error::myerror::{MyError, MyResult},
    events::{Envelope, domain_of, stream_filter},
};

use crate::await_version::{Applied, VersionAt};
use crate::health::Readiness;

/// Applies a stream's events to this instance's own database.
///
/// Implementations must be **deterministic**: no `time::now()`, no `rand`, no
/// database-generated ids. Every replica replays the same events independently, so
/// anything derived from a local clock or a random source makes them disagree
/// permanently. Timestamps and ids come from the event.
///
/// The two usual ways to break that are gone by construction here: there is no cursor to
/// forget — the durable consumer holds the position, so an unacked event is simply
/// redelivered — and no clock to reach for other than the `at` handed in. Multi-statement
/// applies are genuinely atomic, rather than a hand-rolled `BEGIN … COMMIT` in one query
/// string.
///
/// Run one with `bus::projector::run(js, Arc::new(MyProjector), db, readiness)`,
/// which wraps it in a [`Tx`] per partition lane.
pub trait Projector: Send + Sync + 'static {
    const STREAM: &'static str;

    /// Durable consumer **prefix**, identical across every replica of **this
    /// service** and unique to this (service, stream) pair — `"view-bookings"`,
    /// `"payment-bookings"`.
    ///
    /// The actual consumers are one per partition, named `"view-bookings-p07"` by
    /// [`durable_name`]. The shared name is what makes replicas share one cursor per
    /// partition instead of each keeping their own; the uniqueness is what stops two
    /// *different* services stealing each other's messages. view-service and
    /// payment-service both project BOOKINGS, and one durable prefix between them
    /// would give each about half the events.
    ///
    /// Changing it creates a *new* set of consumers, which under
    /// [`DeliverPolicy::All`] replays everything still in the stream into an
    /// already-populated database. Idempotent applies make that survivable, not free.
    const DURABLE: &'static str;

    /// The event enum carried by this stream's envelopes.
    type Event: serde::de::DeserializeOwned + Send;

    /// This service's `version_of_at`, from its `bus::version_reader!`.
    ///
    /// [`Tx::apply`] calls it to read the target aggregate's stored version under the
    /// same lock and the same transaction as the apply that may follow — which is what
    /// lets this projector refuse an event that is not the next one. See [`decide`].
    ///
    /// A `const` on the trait rather than a field on [`Tx`] because every projector in a
    /// service shares the one generated function, and the aggregate it is asked about is
    /// the envelope's rather than anything this impl chooses.
    const VERSION_AT: VersionAt;

    /// Apply one decoded event inside an open transaction. Must be idempotent.
    ///
    /// `at` is the envelope's `occurred_at` — the only clock an implementation may
    /// use, because every replica has to derive the same timestamps from the same
    /// event.
    ///
    /// `version` is the aggregate's version *after* the owning service applied
    /// this event — `envelope.version`, not a stream position.
    ///
    /// Store it on the row. It is what a client waits on (`user:<id>@7`), and
    /// holding it lets a projector notice a gap: version 5 arriving on a row at 3
    /// means 4 was missed, which a bare `WHERE status IN $from` would have
    /// absorbed silently.
    ///
    /// This replaced a `seq: u64` carrying the stream sequence. Its only consumer was
    /// booking-service storing a per-spot `bookings_seq` as a compare-and-swap
    /// precondition — a column that is now gone entirely; see
    /// `shared::domain_models::booking::SpotMirror`.
    ///
    /// Takes `&mut AsyncPgConnection`, which IS the open transaction: [`Tx::apply`]
    /// owns the begin and the commit and this only ever sees the connection inside
    /// them. A projector arm routinely writes a row and then a version, so it needs a
    /// handle it can issue several statements against rather than one consumed per
    /// statement.
    fn apply(
        &self,
        conn: &mut AsyncPgConnection,
        event: Self::Event,
        at: DateTime<Utc>,
        version: i64,
    ) -> impl Future<Output = MyResult<()>> + Send;
}

/// How long a parked event waits before JetStream hands it back.
///
/// Short, and paired with a high [`MAX_GAP_WAIT`] rather than the other way round. Four
/// budgets sit on top of this one and three of them are small:
///
/// - `await_version::TIMEOUT` is **2s**. A client that echoes `booking:<id>@3` while v3
///   is parked waits out its whole budget and is served stale, so a park has to be much
///   shorter than one round of it to be invisible.
/// - payment-service's `BookingWorker` waits **5s** for its own mirror
///   (`PROJECTION_WAIT`) before deciding whether to move money.
/// - Readiness is a one-way latch (`health::Readiness::mark_caught_up`), so a park can
///   delay a replica joining rotation but cannot 503 one already in it. Still, a cold
///   consumer over a stream whose early history has aged out pays this per aggregate.
/// - The gap actually being gated against is two relays publishing out of order, which
///   is milliseconds.
///
/// So: 200ms × 30 ≈ 6s of cover. Deliberately not `worker`'s 30s × 5 — that is a retry
/// policy for a failing side effect, and this is a wait for a message already in flight.
const NAK_DELAY: Duration = Duration::from_millis(200);

/// How many deliveries a gap gets before it is applied anyway.
///
/// The escape hatch is not optional. Versions are gapless *per aggregate*, but two paths
/// bake a real, permanent gap into the data: two replicas sweeping one booking, and
/// `SpotProjector::cancel`, both of which mint a deterministic event id and then let
/// `outbox::enqueue`'s `ON CONFLICT DO UPDATE` overwrite the pending row at a higher
/// version, so the lower one is never published. The streams also expire, so a consumer
/// created after an aggregate's early events aged out legitimately sees its first event
/// at version 5.
///
/// Without a hatch each of those wedges an aggregate for ever. With it, this is strictly
/// better than what it replaces: it waits ~6s for the gap to close and otherwise does
/// exactly what `set_version!` did on its own — log at error and apply.
/// `i64` to match `message::Info::delivered`, which is signed because the protocol says
/// so rather than because a delivery count can be negative.
const MAX_GAP_WAIT: i64 = 30;

/// What to do with one event, given what the target row already holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// The next event for this aggregate, or one there is no basis to refuse. Apply it
    /// and ack.
    Apply,
    /// Already applied — a redelivery, or a duplicate published outside the stream's
    /// `duplicate_window`. Ack without applying.
    Skip,
    /// An event from the aggregate's future: the one before it has not been applied
    /// here. Nak it and let JetStream bring it back once the gap has had time to close.
    Park,
}

/// Whether an event is the next one for its aggregate.
///
/// **This is where ordering lives now.** It used to be a property of the transport — one
/// relay, elected by `bus::lease`, and one ordered lane per partition with
/// `max_ack_pending: 1` — and both of those existed for nothing else. Ordering is a
/// property of the data instead: every row carries the gapless per-aggregate version its
/// owning service assigned inside the writing transaction, so "is this next?" is a
/// question the projection can answer about itself, and an event that arrives early is
/// handed back to JetStream rather than applied out of order.
///
/// That is also the answer to "where do I buffer it?" — nowhere. A Nak'd message is held
/// by the server, durably, shared across replicas, and redelivered on a timer. An
/// in-memory park queue would be a worse copy of it that dies with the pod.
///
/// Pure, and the reason it is: every branch below is otherwise silent. `Skip` and `Park`
/// write nothing and log nothing in the ordinary case, which is exactly the kind of rule
/// that rots undetected without a table of assertions against it.
///
/// - `backfill` bypasses everything. `outbox::backfill` re-emits an aggregate's whole
///   chain — `Created`, `Confirmed`, `Cancelled` — **all carrying the same version**,
///   because the chain exists to satisfy the downstream `WHERE status IN [...]` guards
///   rather than to describe a version history. Gated, steps two and three would be
///   `Skip`ped and a rebuilt projection would leave every cancelled booking `reserved`.
/// - [`Applied::Unavailable`] is not a fault: it means this database does not store the
///   aggregate, so there is no version to be next to and nothing to refuse.
/// - [`Applied::Pending`] is a row that does not exist. Only a create belongs there.
/// - `incoming <= stored` is a redelivery or a duplicate, which is the common case after
///   a relay restart and must stay cheap.
fn decide(stored: Applied, incoming: i64, delivered: i64, backfill: bool) -> Decision {
    if backfill {
        return Decision::Apply;
    }

    match stored {
        Applied::Unavailable => Decision::Apply,
        // `<= 1` rather than `== 1` so a version below the first one a create can carry
        // is applied rather than parked for ever against a row that will never exist.
        Applied::Pending if incoming <= 1 => Decision::Apply,
        Applied::At(stored) if incoming == stored + 1 => Decision::Apply,
        Applied::At(stored) if incoming <= stored => Decision::Skip,
        // Everything left is a gap: a non-create on a missing row, or a version more
        // than one above what is stored.
        _ if delivered >= MAX_GAP_WAIT => Decision::Apply,
        _ => Decision::Park,
    }
}

/// Drives a [`Projector`], owning the two things the inner type is then unable to get
/// wrong: decoding the envelope, and opening and closing the transaction.
///
/// One of these per **partition lane**, built by [`run`]. `Arc<P>` because the sixteen
/// lanes share one projector — it holds no state, by trait contract, so there is
/// nothing for them to contend over.
pub struct Tx<P> {
    projector: Arc<P>,
    /// The service's one connection, shared by every lane of every projector and by
    /// the request handlers besides.
    ///
    /// A `bb8::Pool`, which is `Arc` inside — so holding one per lane is a refcount
    /// bump. Each event's transaction borrows a connection for its lifetime and returns
    /// it on commit.
    ///
    /// This was `Arc<Surreal<Client>>`, and the distinction it needed explaining for
    /// is gone: `Surreal::begin` consumed its client, so sixteen concurrent
    /// transactions needed sixteen *cloned* handles, and a clone minted a session and
    /// replayed a sign-in onto it at ~27.5ms a time. A pool hands out a connection.
    db: Db,
}

impl<P> Tx<P> {
    /// Holds the pool rather than borrowing one from `projector`: this is the only
    /// place a transaction can be opened, so this is the only thing that needs one. A
    /// `Projector` therefore holds none at all and cannot reach the database except
    /// through the connection it is handed.
    pub fn new(projector: Arc<P>, db: Db) -> Self {
        Self { projector, db }
    }
}

impl<P: Projector> Tx<P> {
    /// Decode and apply, inside one transaction.
    ///
    /// There is no cursor to advance any more. It used to be bumped here, in the
    /// same transaction as the data, because each replica owned a private database
    /// and had to record where *its own copy* had reached — a cursor written
    /// separately could end up ahead of the rows and silently skip events on
    /// restart. Every replica now shares one database and one durable consumer, so
    /// the position lives in NATS: a message that is not acked is redelivered, and
    /// applies are idempotent.
    ///
    /// `&self` — there is no parked client left to protect. The lane still applies
    /// strictly one event at a time, but that is the consumer's `max_ack_pending: 1`
    /// rather than anything this type owns.
    ///
    /// Returns what the lane should do with the message. The version gate ([`decide`])
    /// runs **inside** the transaction and **before** the apply, so the `FOR UPDATE` it
    /// takes on the aggregate's row is still held when the apply writes — which is what
    /// stops two replicas both reading an aggregate at v1 and both concluding they hold
    /// its v2. A `Skip` or a `Park` writes nothing, so its transaction is read-only and
    /// commits rather than rolling back; committing is what releases the lock.
    async fn apply(&self, payload: &[u8], seq: u64, delivered: i64) -> MyResult<Decision> {
        let envelope: Envelope<P::Event> = serde_json::from_slice(payload).map_err(|e| {
            shared::error::myerror::MyError::Bus(format!("decode {} at seq {seq}: {e}", P::STREAM))
        })?;
        let at = envelope.occurred_at;

        // A connection of its own for this event, borrowed from the pool and returned
        // when the transaction ends.
        let mut conn = self
            .db
            .get()
            .await
            .map_err(|e| shared::error::myerror::MyError::Pool(e.to_string()))?;

        // The envelope's version, not the stream sequence: what the projector stores
        // has to be the number the owning service assigned and the client is waiting
        // on.
        let version = envelope.version;
        let projector = self.projector.clone();
        let payload = envelope.payload;
        let backfill = envelope.backfill;

        // `"booking:019f…"` -> `("booking", <uuid>)`. Every publisher builds this from
        // the real aggregate rather than from the subject key, which is why the gate can
        // use it: `bookings.spot.<spot_id>` carries `booking:<id>`, and
        // `payments.payout.<host_id>` carries `payout:<id>`.
        //
        // An envelope whose aggregate does not parse is not a reason to stop — there is
        // simply nothing to gate on, so it takes the path an aggregate this database does
        // not store takes.
        let aggregate = shared::events::split_aggregate(&envelope.aggregate)
            .map(|(table, id)| (table.to_string(), id));
        let name = envelope.aggregate;

        // `transaction` owns the begin, the commit and the rollback: returning `Err`
        // from the closure rolls back, returning `Ok` commits. That replaces the
        // explicit begin/commit/rollback this used to spell out — the branches are
        // gone because there is no longer a path that can forget one.
        //
        // `scope_boxed` is required by the signature, which cannot be generic over an
        // arbitrary future without boxing it (rustc#100013, cited in diesel-async).
        let decision = conn
            .transaction::<Decision, shared::error::myerror::MyError, _>(|conn| {
                async move {
                    let stored = match &aggregate {
                        Some((table, id)) => (P::VERSION_AT)(conn, table, id).await?,
                        None => Applied::Unavailable,
                    };

                    let decision = decide(stored, version, delivered, backfill);

                    // The escape hatch fired: this is a gap that did not close in
                    // `MAX_GAP_WAIT` redeliveries. Same error `set_version!` used to log
                    // on its own, minus every case that was really just an event
                    // arriving early — those are a `Park` now and never get here.
                    if decision == Decision::Apply && !backfill && delivered >= MAX_GAP_WAIT {
                        tracing::error!(
                            stream = P::STREAM,
                            aggregate = %name,
                            stored = ?stored,
                            got = version,
                            delivered,
                            "version gap did not close: applying anyway, projection may be incomplete"
                        );
                    }

                    if decision == Decision::Apply {
                        projector.apply(conn, payload, at, version).await?;
                    }

                    Ok(decision)
                }
                .scope_boxed()
            })
            .await?;

        Ok(decision)
    }
}

/// How long a message may be in flight before NATS assumes we died and
/// redelivers it. Must comfortably exceed the slowest `apply` — a redelivery
/// while the first attempt is still committing means two instances writing the
/// same rows, which TiKV refuses rather than corrupts, but which is still churn
/// nobody wants.
const ACK_WAIT: Duration = Duration::from_secs(30);

/// How often readiness is refreshed from the consumer's own state.
///
/// Back to 250ms. It was raised to a second when a tick cost `PARTITIONS`
/// `consumer.info()` round trips — ~64 requests a second per projector, for a number
/// `/readyz` reads at human speed. One consumer per stream makes it one request again.
const POLL: Duration = Duration::from_millis(250);

/// Runs a projector until every lane has stopped. Spawn one per stream.
///
/// On an apply error a lane **stops** rather than skipping the message. A skipped
/// event makes this projection permanently disagree with the log, which is far
/// worse than being drained: `/readyz` goes 503 and the pod is taken out of
/// rotation, leaving a diagnosable stopped replica instead of a silently wrong
/// one.
///
/// ## Ordered per aggregate, concurrent across them
///
/// Every replica of a service shares one database, so applying the same event on
/// all of them would be N writers racing every row. The consumer is therefore
/// **durable and shared** — every replica pulls from one cursor, so each event is
/// applied exactly once by whichever replica picked it up.
///
/// Work-sharing normally costs ordering, which this projection cannot afford:
/// downstream guards read `WHERE status IN $from` and *drop* a transition that
/// arrives before the state it expects. [`decide`] is what buys it back, and it buys
/// it from the data rather than from the transport — an event whose predecessor is
/// not applied here yet is handed back to JetStream instead of being applied early.
///
/// That job used to belong to `max_ack_pending: 1` across `PARTITIONS` consumers:
/// sixteen ordered lanes per stream, each withholding its next message until the last
/// was acked, so a service projecting four streams held sixty-four consumers open and
/// paid a round trip per event. The ceiling was sixteen concurrent applies per stream
/// *across the whole deployment*, no matter how many replicas ran. One consumer with a
/// real in-flight window replaces all of it, and the aggregate's version decides the
/// order.
///
/// ## What this is not
///
/// Not an ownership protocol. Every replica pulls from the same durable and JetStream
/// hands each message to whichever puller asked first, so work distributes itself and
/// a dead replica's in-flight messages are picked up by another after `ACK_WAIT` with
/// nothing to coordinate.
pub async fn run<P: Projector>(js: Context, projector: Arc<P>, db: Db, readiness: Arc<Readiness>) {
    // Up front, so a projector that cannot declare its consumer fails here rather than
    // inside the apply loop.
    let consumer = match stream_consumer::<P>(&js).await {
        Ok(consumer) => consumer,
        Err(e) => {
            tracing::error!(stream = P::STREAM, durable = P::DURABLE, error = %e, "projector could not start");
            readiness.mark_failed(P::STREAM);
            return;
        }
    };

    // Readiness is reported from the consumer rather than from this instance's own
    // progress. With the events shared out between replicas, "how far have *I* got"
    // is not a fact about the database any more.
    tokio::spawn(report_readiness::<P>(consumer.clone(), readiness.clone()));

    tracing::info!(
        stream = P::STREAM,
        durable = P::DURABLE,
        concurrency = APPLY_CONCURRENCY,
        "projecting"
    );

    let tx = Tx::new(projector, db);
    match lane::<P>(consumer, tx).await {
        Err(e) => {
            tracing::error!(stream = P::STREAM, durable = P::DURABLE, error = %e, "projector stopped")
        }
        Ok(()) => tracing::error!(stream = P::STREAM, "projector ended without error"),
    }
    readiness.mark_failed(P::STREAM);
}

/// Declares this projector's one durable consumer.
async fn stream_consumer<P: Projector>(js: &Context) -> MyResult<PullConsumer> {
    let domain = domain_of(P::STREAM)
        .ok_or_else(|| bus_err(format!("{} is not in shared::events::STREAMS", P::STREAM)))?;

    let stream = js
        .get_stream(P::STREAM)
        .await
        .map_err(|e| bus_err(format!("get stream {}: {e}", P::STREAM)))?;

    stream
        .get_or_create_consumer(P::DURABLE, consumer_config(P::DURABLE, domain))
        .await
        .map_err(|e| bus_err(format!("create consumer {}: {e}", P::DURABLE)))
}

/// How many events this replica applies at once.
///
/// Bounded by the connection pool rather than by the broker: `shared::db` builds one
/// pool per process and view-service runs four projectors on it, so this many times
/// four is the standing demand for connections from projectors alone, before a single
/// request handler asks for one. A pool timeout surfaces as `MyError::Pool`, stops the
/// projector and 503s the pod, so the cheap failure is being too low.
///
/// Ordering does not constrain it. Two events for one aggregate applied concurrently
/// serialise on the `FOR UPDATE` [`Projector::VERSION_AT`] takes, and the loser reads
/// the winner's version and parks.
const APPLY_CONCURRENCY: usize = 4;

/// Applies and acks. Ordering comes from [`decide`], not from this loop.
///
/// `for_each_concurrent` rather than a `while let`: with the gate holding the order,
/// a sequential loop would make one replica apply one event at a time — strictly worse
/// than the sixteen lanes it replaces. The bound is [`APPLY_CONCURRENCY`].
///
/// Deliberately **not** grouped by aggregate. A keyed scheduler is the in-memory park
/// queue this design exists to avoid, wearing a different hat; the `FOR UPDATE` in the
/// gate already is the grouping, and it works across replicas rather than within one.
async fn lane<P: Projector>(consumer: PullConsumer, projector: Tx<P>) -> MyResult<()> {
    let messages = consumer
        .messages()
        .await
        .map_err(|e| bus_err(format!("consume: {e}")))?;

    messages
        .map(|msg| msg.map_err(|e| bus_err(format!("next message: {e}"))))
        .try_for_each_concurrent(APPLY_CONCURRENCY, |msg| {
            let projector = &projector;
            async move {
                let info = msg
                    .info()
                    .map_err(|e| bus_err(format!("message info: {e}")))?;
                let (seq, delivered) = (info.stream_sequence, info.delivered);

                match projector.apply(&msg.payload, seq, delivered).await? {
                    // Only after the transaction committed. A crash in this gap leaves
                    // the message unacked, so it is redelivered and reapplied — which is
                    // safe precisely because every `apply` is idempotent, and is the
                    // reason there is no longer a cursor to keep in step with the rows.
                    //
                    // `Skip` acks for the same reason it did not apply: the version it
                    // carries is already stored, so redelivering it for ever would
                    // achieve nothing.
                    Decision::Apply | Decision::Skip => msg
                        .ack()
                        .await
                        .map_err(|e| bus_err(format!("ack {seq}: {e}")))?,

                    // Hand it back and let the server hold it. This is the whole of the
                    // "buffer it until the versions line up" mechanism — durable, shared
                    // across replicas, and already built.
                    Decision::Park => msg
                        .ack_with(async_nats::jetstream::AckKind::Nak(Some(NAK_DELAY)))
                        .await
                        .map_err(|e| bus_err(format!("nak {seq}: {e}")))?,
                }
                Ok(())
            }
        })
        .await?;

    Err(bus_err("message stream ended".to_string()))
}

/// Reports to [`Readiness`] on a timer, from every lane at once.
///
/// "Caught up" means **all** partitions are drained. Any other rule lets a replica
/// serve reads while one lane is still replaying, which is exactly the state
/// `/readyz` exists to keep out of rotation.
///
/// There is no per-stream "applied sequence" any more. It used to publish
/// `ack_floor.stream_sequence` from the one consumer; with sixteen cursors per stream
/// that number has no single value, and its only reader now asks a better question —
/// see `bus::await_version::reached`.
async fn report_readiness<P: Projector>(consumer: PullConsumer, readiness: Arc<Readiness>) {
    let mut consumer = consumer;
    loop {
        match consumer.info().await {
            // Not "sequence >= the head I saw at boot" — that number is this
            // instance's guess, and on a shared consumer it can be reached by
            // someone else's work or never reached at all if the head message
            // has since been deleted.
            Ok(info) if info.num_pending == 0 && info.num_ack_pending == 0 => {
                readiness.mark_caught_up(P::STREAM)
            }
            Ok(_) => {}
            Err(e) => tracing::debug!(stream = P::STREAM, error = %e, "consumer info failed"),
        }
        tokio::time::sleep(POLL).await;
    }
}

/// Split out so the settings that define this primitive can be asserted without a
/// broker. Four of the five are silent when wrong.
fn consumer_config(durable: &str, domain: &str) -> async_nats::jetstream::consumer::pull::Config {
    async_nats::jetstream::consumer::pull::Config {
        durable_name: Some(durable.to_string()),
        // The whole bounded context. This was one consumer per partition filtering
        // `<domain>.7.>`, sixteen of them, and the partitioning existed only to claw
        // back the concurrency `max_ack_pending: 1` gave away.
        filter_subject: stream_filter(domain),
        // Honoured only when the consumer is first created; afterwards the stored
        // position wins. `All` is what makes a brand-new projection build itself
        // from the whole log — the opposite of a `Worker`, which starts at `New`
        // precisely so deploying it does not re-send every email ever.
        deliver_policy: DeliverPolicy::All,
        ack_policy: AckPolicy::Explicit,
        ack_wait: ACK_WAIT,
        // **This used to be 1, and that was the ordering guarantee.** One unacked
        // message per consumer meant the server withheld the next one until the last
        // was acked, which serialised applies across every replica — a round trip per
        // event, and sixteen consumers per stream to get any concurrency back.
        //
        // [`decide`] holds the ordering now, so the transport does not have to. Raising
        // this is not merely an optimisation either: with 1, a Nak'd message stays the
        // single message in flight and is redelivered ahead of everything else, so the
        // event that would close the gap can never arrive and the park livelocks until
        // the escape hatch fires. The gate and this number are one change.
        //
        // 64 rather than something larger: it is the in-flight window, and every
        // message in it can be mid-apply holding a pool connection.
        max_ack_pending: 64,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Guards the settings that make this safe on a shared database, all of which
    /// fail silently: without a `durable_name` every replica gets every event and they
    /// race each other over every row; with `max_ack_pending: 1` a parked event is the
    /// only message in flight and the event that would close its gap can never arrive;
    /// with the wrong `filter_subject` the consumer sits at zero pending for ever.
    #[test]
    fn the_consumer_is_durable_shared_and_has_a_real_window() {
        let config = consumer_config("view-bookings", "bookings");

        assert_eq!(
            config.durable_name.as_deref(),
            Some("view-bookings"),
            "an ephemeral consumer makes this fan-out: every replica applies every event"
        );
        assert_eq!(
            config.filter_subject, "bookings.>",
            "the whole bounded context; a narrower filter silently consumes nothing"
        );
        assert!(
            config.max_ack_pending > 1,
            "with one in flight, a Nak'd event is redelivered ahead of everything else \
             and the event that would close its gap never arrives — the park livelocks \
             until the escape hatch fires"
        );
        assert!(
            matches!(config.deliver_policy, DeliverPolicy::All),
            "a projection must build from the whole log, unlike a Worker's DeliverPolicy::New"
        );
    }

    /// The in-flight window may exceed the apply concurrency — that is what keeps the
    /// pulling ahead of the applying — but the concurrency must not exceed the window,
    /// which would be slots that can never be filled.
    #[test]
    fn the_window_is_at_least_the_apply_concurrency() {
        let config = consumer_config("view-bookings", "bookings");
        assert!(config.max_ack_pending as usize >= APPLY_CONCURRENCY);
    }

    /// The ordering rule, branch by branch.
    ///
    /// This table is the reason [`decide`] is a pure function. Two of its three verdicts
    /// write nothing and log nothing — a `Skip` looks exactly like an apply that happened
    /// to change no columns, and a `Park` looks exactly like a quiet lane — so a wrong
    /// branch here is invisible everywhere else until a projection is silently short an
    /// event.
    #[test]
    fn only_the_next_version_applies() {
        use Applied::{At, Pending, Unavailable};
        use Decision::{Apply, Park, Skip};

        // The ordinary path.
        assert_eq!(decide(Pending, 1, 1, false), Apply, "the create");
        assert_eq!(decide(At(3), 4, 1, false), Apply, "the next event");

        // Already applied. The common case after a relay restart republishes its tail,
        // and it must stay cheap rather than becoming a redelivery loop.
        assert_eq!(decide(At(3), 3, 1, false), Skip, "a redelivery");
        assert_eq!(decide(At(3), 2, 1, false), Skip, "an out-of-order duplicate");

        // Early. This is the whole point: v5 on a row at v3 means v4 has not been
        // applied here yet, so it goes back to the server rather than landing on top.
        assert_eq!(decide(At(3), 5, 1, false), Park, "one missing");
        assert_eq!(decide(Pending, 4, 1, false), Park, "a non-create on no row");

        // The escape hatch. A gap that survives this many deliveries is not an event in
        // flight — it is one that was never published — so it degrades to exactly what
        // `set_version!` did before the gate existed.
        assert_eq!(decide(At(3), 5, MAX_GAP_WAIT, false), Apply, "gap gave up");
        assert_eq!(
            decide(At(3), 5, MAX_GAP_WAIT - 1, false),
            Park,
            "one delivery short of the budget still waits"
        );

        // Backfill re-emits a whole chain at ONE version — `Created`, `Confirmed`,
        // `Cancelled` all at v7 — because the chain exists to satisfy the downstream
        // status guards, not to describe a version history. Gated, steps two and three
        // would `Skip` and a rebuilt projection would leave the booking `reserved`.
        assert_eq!(decide(At(7), 7, 1, true), Apply, "backfill step two");
        assert_eq!(decide(At(9), 7, 1, true), Apply, "backfill onto a newer row");
        assert_eq!(decide(Pending, 7, 1, true), Apply, "backfill onto no row");

        // Nothing to be next to: this database does not store the aggregate, or the
        // envelope's `aggregate` did not parse. Neither is a fault and neither is a
        // reason to hold the message.
        assert_eq!(decide(Unavailable, 9, 1, false), Apply, "not stored here");

        // A version at or below the first one a create can carry must not park against
        // a row that will never exist.
        assert_eq!(decide(Pending, 0, 1, false), Apply, "version zero");
    }

    /// Every stream a projector can name has to resolve to a subject domain, or
    /// `run` refuses to start it.
    #[test]
    fn every_stream_has_a_domain() {
        for (stream, ..) in shared::events::STREAMS {
            assert!(domain_of(stream).is_some(), "{stream}");
        }
        assert!(domain_of("NOT_A_STREAM").is_none());
    }
}

fn bus_err(msg: String) -> MyError {
    MyError::Bus(msg)
}

/// The properties the partitioning exists for, against a real broker.
///
/// None of these can be asserted without one: the partition assignment happens
/// inside NATS, and so do redelivery, failover and dedupe. The unit tests above
/// cover the config that reaches it; these cover what it then does.
///
/// ```sh
/// docker compose -f docker/docker-compose-dev.yml up -d
/// cargo test --workspace -- --ignored
/// ```
///
/// `#[ignore]`d and pointed at the dev stack, like the `live_tests` modules in the
/// service repositories — CI has no broker.
///
/// **They run on the real SESSIONS stream**, which is the one stream nothing in this
/// codebase consumes, so a test's events cannot wake a real projector or worker. Each
/// test owns a durable prefix, deletes its lanes on the way in so `DeliverPolicy::All`
/// starts clean, and keys every event with a uuid minted for that run — so a replay of
/// an earlier run's leftovers is recorded and then filtered out rather than confusing
/// an assertion.
#[cfg(test)]
mod live_tests {
    use std::{
        collections::HashSet,
        sync::Mutex,
        time::Instant,
    };

    use async_nats::jetstream::message::PublishMessage;
    use diesel_async::RunQueryDsl;
    use shared::events::{Envelope, STREAM_SESSIONS, aggregate_id, session_subject};
    use uuid::Uuid;

    use super::*;

    const NATS: &str = "nats://127.0.0.1:4222";
    const URL: &str = "postgres://yugabyte@127.0.0.1:5433/view";
    /// Any database with a connection; the scratch table is created below.
    // The database name is part of `URL` above now, rather than a separate argument to
    // `connect` — one `DATABASE_URL` per service replaced addr/user/pass/db.
    /// Written by these tests alone, and defined up front — sixteen lanes creating it
    /// implicitly would all write the same table-definition key and take a TiKV write
    /// conflict. An earlier spike learned that the hard way.
    ///
    /// **Two leading underscores on purpose.** It is never dropped — the eight live tests
    /// run concurrently, so a teardown in any one of them would pull the table out from
    /// under the others, which is the same race `define_scratch_table` retries around.
    /// So it survives the run and would be picked up by `scripts/print-schema.sh` as if
    /// it were a real table. `diesel print-schema` skips `__%` (its table listing filters
    /// `NOT LIKE '\_\_%'`), which is the same reason `__diesel_schema_migrations` needs no
    /// entry in `diesel.toml`.
    const TABLE: &str = "__bus_livetest";

    /// How long a recorded apply holds its lane open.
    ///
    /// Long enough that two lanes running at once overlap observably, and that two
    /// events in one lane cannot appear to.
    const HOLD: Duration = Duration::from_millis(400);

    /// One applied event, with the window it occupied.
    #[derive(Clone, Debug)]
    struct Applied {
        key: Uuid,
        version: i64,
        entered: Instant,
        exited: Instant,
    }

    impl Applied {
        fn overlaps(&self, other: &Applied) -> bool {
            self.entered < other.exited && other.entered < self.exited
        }
    }

    /// A projector that records rather than projects — plus the one real write that
    /// makes idempotence observable.
    struct Recorder {
        log: Mutex<Vec<Applied>>,
        /// How long to hold a lane open, and **only for `watching`**.
        ///
        /// Holding every key would make each test pay for the whole stream: these
        /// lanes are `DeliverPolicy::All` on a shared SESSIONS stream, so they replay
        /// every earlier run's leftovers before reaching this run's keys. Unwatched
        /// keys are applied instantly and filtered out of the assertions.
        hold: Duration,
        watching: Mutex<HashSet<Uuid>>,
        /// Counts entries into `record` for a watched key, so a test can wait for a
        /// lane to be genuinely mid-apply rather than merely started.
        entered: Mutex<usize>,
        /// Keys still owed one failure. Returning an error *after* the work is the
        /// closest faithful simulation of dying before the ack: the transaction is
        /// rolled back, the message is never acked, and JetStream redelivers.
        fail_once: Mutex<HashSet<Uuid>>,
    }

    impl Recorder {
        fn new(hold: Duration, watching: impl IntoIterator<Item = Uuid>) -> Arc<Self> {
            Arc::new(Self {
                log: Mutex::new(Vec::new()),
                hold,
                watching: Mutex::new(watching.into_iter().collect()),
                entered: Mutex::new(0),
                fail_once: Mutex::new(HashSet::new()),
            })
        }

        fn failing_once_on(key: Uuid) -> Arc<Self> {
            let recorder = Self::new(Duration::ZERO, [key]);
            recorder.fail_once.lock().unwrap().insert(key);
            recorder
        }

        fn entered(&self) -> usize {
            *self.entered.lock().unwrap()
        }

        /// Everything this run applied for `key`, oldest first.
        fn applies(&self, key: Uuid) -> Vec<Applied> {
            let log = self.log.lock().unwrap();
            log.iter().filter(|a| a.key == key).cloned().collect()
        }

        fn versions(&self, key: Uuid) -> Vec<i64> {
            self.applies(key).iter().map(|a| a.version).collect()
        }

        async fn record(
            &self,
            conn: &mut AsyncPgConnection,
            event: serde_json::Value,
            version: i64,
        ) -> MyResult<()> {
            // Not one of ours: a real session event from dev traffic, a leftover from
            // an earlier run, or — the case that actually bites — another test's key.
            // Every test's lanes are `DeliverPolicy::All` on the one SESSIONS stream,
            // so all of them see all of it.
            //
            // Skipped entirely rather than applied-and-filtered. Writing the row for
            // another test's key made `redelivery_is_idempotent` see a row it had just
            // rolled back, because a *different* test's recorder had written it; and
            // applying every key made each test replay the whole stream through a
            // transaction apiece, which is what timed the slower two out. The lane
            // still returns `Ok` and acks — refusing would stop it.
            let key = event
                .get("key")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<Uuid>().ok());
            let Some(key) = key.filter(|k| self.watching.lock().unwrap().contains(k)) else {
                return Ok(());
            };

            *self.entered.lock().unwrap() += 1;
            let entered = Instant::now();

            diesel::sql_query(format!(
                "INSERT INTO {TABLE} (id, applies, version) VALUES ($1, 1, 0)
                 ON CONFLICT (id) DO UPDATE SET applies = {TABLE}.applies + 1"
            ))
            .bind::<diesel::sql_types::Uuid, _>(key)
            .execute(&mut *conn)
            .await?;

            // The same statement `shared::db::set_version` issues, written out rather
            // than called. That function resolves its table through
            // `shared::db::table_for`, which is an allowlist of the aggregates the
            // services own — and a scratch table that exists only for these tests has
            // no business being in it. The gap *logic* it also carries is pure and is
            // covered by `version_gap`'s unit tests; what these lanes need is the
            // `WHERE version < $2` guard, which is right here.
            diesel::sql_query(format!(
                "UPDATE {TABLE} SET version = $2 WHERE id = $1 AND version < $2"
            ))
            .bind::<diesel::sql_types::Uuid, _>(key)
            .bind::<diesel::sql_types::BigInt, _>(version)
            .execute(&mut *conn)
            .await?;

            if self.fail_once.lock().unwrap().remove(&key) {
                return Err(MyError::Bus(format!(
                    "simulated crash after applying {key}@{version}, before the ack"
                )));
            }

            tokio::time::sleep(self.hold).await;

            self.log.lock().unwrap().push(Applied {
                key,
                version,
                entered,
                exited: Instant::now(),
            });
            Ok(())
        }
    }

    /// The test lanes' [`VersionAt`] — the one hand-written one in the codebase.
    ///
    /// Every real service gets this from `bus::version_reader!`, against tables its own
    /// schema module declares. The scratch table has no schema module (and deliberately
    /// no entry in `table_for`'s allowlist, for the reason `record` spells out), so the
    /// gate's read is written out here the same way `record`'s two writes are.
    ///
    /// The aggregate name is ignored: these lanes project exactly one thing, and the
    /// envelope's `session:<key>` id is the scratch table's primary key.
    fn scratch_version_at<'a>(
        conn: &'a mut AsyncPgConnection,
        _aggregate: &'a str,
        id: &'a Uuid,
    ) -> futures::future::BoxFuture<'a, MyResult<crate::await_version::Applied>> {
        use crate::await_version::Applied as Stored;
        use diesel::OptionalExtension as _;
        use diesel_async::RunQueryDsl as _;

        Box::pin(async move {
            #[derive(diesel::QueryableByName)]
            struct Row {
                #[diesel(sql_type = diesel::sql_types::BigInt)]
                version: i64,
            }

            let row: Option<Row> =
                diesel::sql_query(format!("SELECT version FROM {TABLE} WHERE id = $1 FOR UPDATE"))
                    .bind::<diesel::sql_types::Uuid, _>(*id)
                    .get_result(conn)
                    .await
                    .optional()?;

            Ok(match row {
                Some(row) => Stored::At(row.version),
                None => Stored::Pending,
            })
        })
    }

    /// `Projector` carries its durable name as an associated const, so one test's
    /// lanes are one type. This mints them.
    macro_rules! recorder {
        ($name:ident, $durable:literal) => {
            struct $name(Arc<Recorder>);

            impl Projector for $name {
                const STREAM: &'static str = STREAM_SESSIONS;
                const DURABLE: &'static str = $durable;
                const VERSION_AT: crate::await_version::VersionAt = scratch_version_at;
                /// `Value`, not a real event enum: these lanes also see whatever else
                /// is on SESSIONS, and a decode failure stops a lane.
                type Event = serde_json::Value;

                async fn apply(
                    &self,
                    conn: &mut AsyncPgConnection,
                    event: serde_json::Value,
                    _at: DateTime<Utc>,
                    version: i64,
                ) -> MyResult<()> {
                    self.0.record(conn, event, version).await
                }
            }
        };
    }

    recorder!(OrderLanes, "bus-lt-order");
    recorder!(ConcurrentLanes, "bus-lt-concurrent");
    recorder!(RedeliveryLanes, "bus-lt-redelivery");
    recorder!(FailoverLanes, "bus-lt-failover");
    recorder!(RestartLanes, "bus-lt-restart");
    recorder!(DuplicateLanes, "bus-lt-duplicate");
    recorder!(GapLanes, "bus-lt-gap");
    recorder!(ParkLanes, "bus-lt-park");
    recorder!(BackfillLanes, "bus-lt-backfill");

    /// Creates the scratch table, tolerating the race between concurrent tests.
    ///
    /// `IF NOT EXISTS` is not a lock, and the retry is still needed — only the error
    /// it absorbs has changed. Two tests starting together both find the table
    /// missing and both try to create it; under TiKV that was a write conflict on the
    /// table-definition key, and in Postgres it surfaces as a unique violation on the
    /// catalogue (23505) or "already exists" (42P07). Retried rather than serialised,
    /// because the loser's retry finds the table there and does nothing.
    async fn define_scratch_table(db: &Db) {
        for attempt in 0..5 {
            let mut conn = db.get().await.expect("a connection");
            let result = diesel::sql_query(format!(
                "CREATE TABLE IF NOT EXISTS {TABLE} (
                     id      uuid PRIMARY KEY,
                     applies bigint NOT NULL DEFAULT 0,
                     version bigint NOT NULL DEFAULT 0
                 )"
            ))
            .execute(&mut *conn)
            .await;

            match result {
                Ok(_) => return,
                // Two lanes racing `CREATE TABLE IF NOT EXISTS` can still collide on the
                // catalogue: 23505 (unique violation on pg_type) or 42P07 (duplicate
                // table). Both mean the other one won, which is the outcome wanted.
                //
                // Matched on `DatabaseErrorKind` now — diesel exposes no SQLSTATE.
                // `UniqueViolation` covers 23505; 42P07 lands in `Unknown`, so the retry
                // is bounded by the attempt count rather than by the code. The loser
                // finds the table on its next pass either way.
                Err(diesel::result::Error::DatabaseError(_, _)) if attempt < 4 => {
                    tokio::time::sleep(Duration::from_millis(100 * (attempt + 1))).await;
                }
                Err(e) => panic!("scratch table: {e}"),
            }
        }
    }

    struct Live {
        js: Context,
        db: Db,
        readiness: Arc<Readiness>,
    }

    /// Empties SESSIONS once per test binary, before any test publishes.
    ///
    /// These tests all run `DeliverPolicy::All` against the one stream nothing in the
    /// codebase consumes, so each of them replays every earlier run's leftovers. That was
    /// merely wasteful when a leftover was applied and filtered out. It is not any more:
    /// `a_gap_that_never_closes…` deliberately leaves a key with a permanent version gap,
    /// and SESSIONS keeps messages for 31 days — so every later consumer parks that key
    /// for the full `NAK_DELAY × MAX_GAP_WAIT` budget, once per leftover, compounding run
    /// over run until the slower tests time out.
    ///
    /// A `OnceCell` rather than a purge per test: every `live()` awaits the same cell, so
    /// the purge is ordered before the first publish of the run rather than racing tests
    /// that have already started.
    async fn purge_once(js: &Context) {
        static PURGED: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();
        PURGED
            .get_or_init(|| async {
                js.get_stream(STREAM_SESSIONS)
                    .await
                    .expect("stream")
                    .purge()
                    .await
                    .expect("purge SESSIONS");
            })
            .await;
    }

    async fn live() -> Live {
        shared::install_default_crypto_provider();

        let js = crate::connect(NATS)
            .await
            .expect("NATS on :4222 — docker compose -f docker/docker-compose-dev.yml up -d");
        crate::ensure_streams(&js).await.expect("declare streams");
        purge_once(&js).await;

        let db = shared::db::connect(URL)
            .await
            .expect("yugabyte on :5433 — same compose file");
        define_scratch_table(&db).await;

        let readiness = Readiness::new(js.client().clone(), &[STREAM_SESSIONS]);
        Live { js, db, readiness }
    }

    impl Live {
        /// Publishes one event and returns the stream sequence NATS stored it at.
        ///
        /// Same shape as `outbox::append`: the envelope encoded, `Nats-Msg-Id` set to
        /// the event id so the stream's `duplicate_window` can see a repeat.
        async fn publish(&self, key: Uuid, version: i64, event_id: Uuid) -> u64 {
            self.publish_as(key, version, event_id, false).await
        }

        /// The same, flagged as a rebuild. `outbox::backfill` emits a whole chain at one
        /// version, so this is the only way to produce several events that are all
        /// legitimately "not the next one".
        async fn publish_backfill(&self, key: Uuid, version: i64, event_id: Uuid) -> u64 {
            self.publish_as(key, version, event_id, true).await
        }

        async fn publish_as(&self, key: Uuid, version: i64, event_id: Uuid, backfill: bool) -> u64 {
            let envelope = Envelope {
                event_id,
                aggregate: aggregate_id("session", &key),
                version,
                occurred_at: Utc::now(),
                actor_id: None,
                backfill,
                // The aggregate again, in the payload: `Projector::apply` is handed
                // the decoded event and not the envelope, so this is how a recorded
                // apply knows which key it was for.
                payload: serde_json::json!({ "key": key.to_string() }),
            };

            self.js
                .send_publish(
                    session_subject(&key),
                    PublishMessage::build()
                        .message_id(event_id.to_string())
                        .payload(serde_json::to_vec(&envelope).expect("encode").into()),
                )
                .await
                .expect("publish")
                .await
                .expect("publish ack")
                .sequence
        }

        /// Removes a test's consumer so `DeliverPolicy::All` is honoured on the next run.
        async fn drop_lanes(&self, durable: &str) {
            let stream = self.js.get_stream(STREAM_SESSIONS).await.expect("stream");
            let _ = stream.delete_consumer(durable).await;
        }

        /// `applies` and `version` as stored, for the idempotence assertions.
        ///
        /// One statement now, where SurrealDB needed two `SELECT VALUE`s — a row is a
        /// tuple here rather than one scalar per query.
        async fn row(&self, key: Uuid) -> Option<(i64, i64)> {
            #[derive(diesel::QueryableByName)]
            struct Row {
                #[diesel(sql_type = diesel::sql_types::BigInt)]
                applies: i64,
                #[diesel(sql_type = diesel::sql_types::BigInt)]
                version: i64,
            }

            let mut conn = self.db.get().await.ok()?;
            let rows: Vec<Row> = diesel::sql_query(format!(
                "SELECT applies, version FROM {TABLE} WHERE id = $1"
            ))
            .bind::<diesel::sql_types::Uuid, _>(key)
            .load(&mut *conn)
            .await
            .ok()?;

            rows.into_iter().next().map(|r| (r.applies, r.version))
        }
    }

    /// Polls until `done`, or panics with `what` after `limit`.
    ///
    /// Everything here waits on a broker round trip, so a fixed sleep would be either
    /// flaky or slow.
    async fn until(limit: Duration, what: &str, mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + limit;
        while Instant::now() < deadline {
            if done() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("timed out after {limit:?} waiting for {what}");
    }

    // ─── the requirements ───────────────────────────────────────────────────

    /// Requirement 1. Three events for one aggregate, applied in order and never
    /// at the same time.
    #[tokio::test]
    #[ignore = "needs docker/docker-compose-dev.yml"]
    async fn same_key_stays_ordered() {
        let live = live().await;
        live.drop_lanes(OrderLanes::DURABLE).await;

        let key = Uuid::now_v7();
        for version in 1..=3 {
            live.publish(key, version, Uuid::now_v7()).await;
        }

        let recorder = Recorder::new(HOLD, [key]);
        let projector = Arc::new(OrderLanes(recorder.clone()));
        let running = tokio::spawn(run(
            live.js.clone(),
            projector,
            live.db.clone(),
            live.readiness.clone(),
        ));

        until(Duration::from_secs(30), "three applies", || {
            recorder.applies(key).len() == 3
        })
        .await;
        running.abort();

        assert_eq!(
            recorder.versions(key),
            vec![1, 2, 3],
            "applied out of order"
        );

        // Ordered is not the same claim as serial, and only the second one rules out
        // two replicas inside the same aggregate at once.
        let applies = recorder.applies(key);
        for pair in applies.windows(2) {
            assert!(
                !pair[0].overlaps(&pair[1]),
                "v{} and v{} overlapped: one aggregate was applied concurrently",
                pair[0].version,
                pair[1].version
            );
        }

        live.drop_lanes(OrderLanes::DURABLE).await;
    }

    /// Requirement 2. Two aggregates are applied at the same time.
    ///
    /// **Any** two, now. This used to have to publish a dozen keys and ask NATS which
    /// partition each had landed in, because two aggregates sharing one of sixteen lanes
    /// were applied strictly one after the other — a real ceiling, and one that no number
    /// of replicas moved. There are no lanes to share: concurrency is
    /// [`APPLY_CONCURRENCY`] per replica and ordering is the version gate's job.
    #[tokio::test]
    #[ignore = "needs docker/docker-compose-dev.yml"]
    async fn different_keys_run_concurrently() {
        let live = live().await;
        live.drop_lanes(ConcurrentLanes::DURABLE).await;

        let (left, right) = (Uuid::now_v7(), Uuid::now_v7());
        live.publish(left, 1, Uuid::now_v7()).await;
        live.publish(right, 1, Uuid::now_v7()).await;

        let recorder = Recorder::new(HOLD, [left, right]);
        let running = tokio::spawn(run(
            live.js.clone(),
            Arc::new(ConcurrentLanes(recorder.clone())),
            live.db.clone(),
            live.readiness.clone(),
        ));

        until(Duration::from_secs(60), "both keys applied", || {
            !recorder.applies(left).is_empty() && !recorder.applies(right).is_empty()
        })
        .await;
        running.abort();

        let (a, b) = (
            recorder.applies(left).remove(0),
            recorder.applies(right).remove(0),
        );
        assert!(
            a.overlaps(&b),
            "two unrelated aggregates ran one after the other, so nothing was parallelised"
        );

        live.drop_lanes(ConcurrentLanes::DURABLE).await;
    }

    /// Requirement 6. Work done, then a failure before the ack. JetStream redelivers
    /// and the projection is unharmed by applying twice.
    #[tokio::test]
    #[ignore = "needs docker/docker-compose-dev.yml"]
    async fn redelivery_is_idempotent() {
        let live = live().await;
        live.drop_lanes(RedeliveryLanes::DURABLE).await;

        let key = Uuid::now_v7();
        live.publish(key, 1, Uuid::now_v7()).await;

        // First instance applies and then fails, so the message is never acked.
        let failing = Recorder::failing_once_on(key);
        let first = tokio::spawn(run(
            live.js.clone(),
            Arc::new(RedeliveryLanes(failing.clone())),
            live.db.clone(),
            live.readiness.clone(),
        ));
        until(Duration::from_secs(30), "the simulated crash", || {
            failing.fail_once.lock().unwrap().is_empty()
        })
        .await;
        first.abort();

        // Whatever the failed attempt wrote was rolled back with its transaction.
        assert_eq!(
            live.row(key).await,
            None,
            "a rolled-back apply left rows behind"
        );

        // Second instance picks the unacked message back up.
        let recovered = Recorder::new(Duration::ZERO, [key]);
        let second = tokio::spawn(run(
            live.js.clone(),
            Arc::new(RedeliveryLanes(recovered.clone())),
            live.db.clone(),
            live.readiness.clone(),
        ));
        until(
            ACK_WAIT + Duration::from_secs(20),
            "redelivery after ack_wait",
            || !recovered.applies(key).is_empty(),
        )
        .await;
        second.abort();

        assert_eq!(recovered.versions(key), vec![1]);
        assert_eq!(
            live.row(key).await,
            Some((1, 1)),
            "the redelivered event should have been applied exactly once"
        );

        live.drop_lanes(RedeliveryLanes::DURABLE).await;
    }

    /// Requirements 3 and 4. Two instances share the durable; killing the one holding a
    /// message hands it to the other with nothing to coordinate.
    ///
    /// Slow by construction — the handover is `ACK_WAIT`, which is what makes it a
    /// failover rather than a graceful drain.
    #[tokio::test]
    #[ignore = "needs docker/docker-compose-dev.yml; takes ~ack_wait"]
    async fn a_dead_instance_hands_its_work_over() {
        let live = live().await;
        live.drop_lanes(FailoverLanes::DURABLE).await;

        let key = Uuid::now_v7();
        live.publish(key, 1, Uuid::now_v7()).await;

        // Holds the message far longer than the test waits, so it is genuinely
        // in flight and unacked when the instance dies.
        let doomed = Recorder::new(Duration::from_secs(600), [key]);
        let one = tokio::spawn(run(
            live.js.clone(),
            Arc::new(FailoverLanes(doomed.clone())),
            live.db.clone(),
            live.readiness.clone(),
        ));

        // Killing it before it has the message would prove nothing — the survivor
        // would just be the first to receive it. `entered` counts entries into the
        // apply, and the 600s hold means it is still inside one.
        until(
            Duration::from_secs(60),
            "the message to be in flight",
            || doomed.entered() > 0,
        )
        .await;
        one.abort();

        // A second instance was never given anything to take over explicitly.
        let survivor = Recorder::new(Duration::ZERO, [key]);
        let two = tokio::spawn(run(
            live.js.clone(),
            Arc::new(FailoverLanes(survivor.clone())),
            live.db.clone(),
            live.readiness.clone(),
        ));

        until(
            ACK_WAIT + Duration::from_secs(30),
            "the survivor to pick the partition up",
            || !survivor.applies(key).is_empty(),
        )
        .await;
        two.abort();

        assert_eq!(survivor.versions(key), vec![1]);
        live.drop_lanes(FailoverLanes::DURABLE).await;
    }

    /// Requirement 5. A restart resumes from the durable cursor: what was acked
    /// before the stop is not applied again, and what arrived while down is not lost.
    #[tokio::test]
    #[ignore = "needs docker/docker-compose-dev.yml"]
    async fn restart_resumes_from_durable_state() {
        let live = live().await;
        live.drop_lanes(RestartLanes::DURABLE).await;

        let key = Uuid::now_v7();
        for version in 1..=3 {
            live.publish(key, version, Uuid::now_v7()).await;
        }

        let before = Recorder::new(Duration::ZERO, [key]);
        let first = tokio::spawn(run(
            live.js.clone(),
            Arc::new(RestartLanes(before.clone())),
            live.db.clone(),
            live.readiness.clone(),
        ));
        until(Duration::from_secs(30), "the first three", || {
            before.applies(key).len() == 3
        })
        .await;
        // Everything is acked, so this is a clean stop rather than a crash — the
        // crash path is `redelivery_is_idempotent` above.
        tokio::time::sleep(Duration::from_millis(500)).await;
        first.abort();

        // Published while nothing was running.
        for version in 4..=5 {
            live.publish(key, version, Uuid::now_v7()).await;
        }

        let after = Recorder::new(Duration::ZERO, [key]);
        let second = tokio::spawn(run(
            live.js.clone(),
            Arc::new(RestartLanes(after.clone())),
            live.db.clone(),
            live.readiness.clone(),
        ));
        // Generous, and for a reason worth knowing rather than a flaky-test fudge.
        // Killing an instance leaves its outstanding pull request on the server until
        // it expires, and JetStream hands the two new messages to that dead inbox
        // first. They are not lost — they are unacked, so they come back on
        // `ACK_WAIT` — but "resumes from durable state" costs a redelivery window
        // when the previous instance was killed rather than drained. A real pod does
        // exactly this.
        until(ACK_WAIT + Duration::from_secs(30), "the last two", || {
            after.applies(key).len() == 2
        })
        .await;
        second.abort();

        assert_eq!(
            after.versions(key),
            vec![4, 5],
            "a restart must resume from the durable cursor, not replay what it acked"
        );
        assert_eq!(
            live.row(key).await.map(|(_, version)| version),
            Some(5),
            "nothing was lost across the restart"
        );

        live.drop_lanes(RestartLanes::DURABLE).await;
    }

    /// Requirement 7. The same `event_id` twice is one message on the stream, so a
    /// republishing relay cannot make a projector see it twice at all.
    #[tokio::test]
    #[ignore = "needs docker/docker-compose-dev.yml"]
    async fn a_duplicate_event_is_discarded_by_the_stream() {
        let live = live().await;
        live.drop_lanes(DuplicateLanes::DURABLE).await;

        let key = Uuid::now_v7();
        let event_id = Uuid::now_v7();

        // Same id, well inside the stream's duplicate_window.
        let first = live.publish(key, 1, event_id).await;
        let second = live.publish(key, 1, event_id).await;
        assert_eq!(
            first, second,
            "the second publish got its own sequence, so Nats-Msg-Id dedupe is off"
        );

        let recorder = Recorder::new(Duration::ZERO, [key]);
        let running = tokio::spawn(run(
            live.js.clone(),
            Arc::new(DuplicateLanes(recorder.clone())),
            live.db.clone(),
            live.readiness.clone(),
        ));
        until(Duration::from_secs(30), "the event", || {
            !recorder.applies(key).is_empty()
        })
        .await;
        // Long enough that a second delivery would have shown up.
        tokio::time::sleep(Duration::from_secs(2)).await;
        running.abort();

        assert_eq!(recorder.versions(key), vec![1]);
        assert_eq!(live.row(key).await, Some((1, 1)));

        live.drop_lanes(DuplicateLanes::DURABLE).await;
    }

    /// **The one that replaced the transport's ordering guarantee.**
    ///
    /// v3 is published *before* v2 — which is exactly what two relays racing produce,
    /// and what `bus::lease` used to exist to prevent. The lane must refuse v3 on a row
    /// at v1, take v2 when it arrives, and apply v3 when JetStream brings it back.
    ///
    /// The old assertion here was `[1, 3, 4]`: the gap applied straight through and the
    /// projection kept v3's columns on top of a row that never saw v2. This is the
    /// inversion of that, and the whole reason the lease and the sixteen lanes can go.
    #[tokio::test]
    #[ignore = "needs docker/docker-compose-dev.yml"]
    async fn an_event_that_arrives_early_is_parked_until_its_predecessor_lands() {
        let live = live().await;
        live.drop_lanes(ParkLanes::DURABLE).await;

        let key = Uuid::now_v7();
        live.publish(key, 1, Uuid::now_v7()).await;
        // Out of order on the wire, deliberately.
        live.publish(key, 3, Uuid::now_v7()).await;
        live.publish(key, 2, Uuid::now_v7()).await;

        let recorder = Recorder::new(Duration::ZERO, [key]);
        let running = tokio::spawn(run(
            live.js.clone(),
            Arc::new(ParkLanes(recorder.clone())),
            live.db.clone(),
            live.readiness.clone(),
        ));
        until(Duration::from_secs(30), "all three events", || {
            recorder.applies(key).len() == 3
        })
        .await;
        running.abort();

        assert_eq!(
            recorder.versions(key),
            vec![1, 2, 3],
            "applied in version order, not in publish order"
        );
        assert_eq!(live.row(key).await.map(|(_, version)| version), Some(3));

        live.drop_lanes(ParkLanes::DURABLE).await;
    }

    /// The escape hatch. A gap that never closes must not wedge the aggregate for ever.
    ///
    /// v2 is genuinely never published — the shape an aged-out history takes, and the
    /// shape two replicas sweeping one booking bake into the data permanently. After
    /// `MAX_GAP_WAIT` deliveries the lane gives up waiting and applies, which is exactly
    /// what happened before the gate existed. The budget is ~6s, so this is the slowest
    /// test here by design.
    #[tokio::test]
    #[ignore = "needs docker/docker-compose-dev.yml"]
    async fn a_gap_that_never_closes_is_applied_rather_than_wedging_the_lane() {
        let live = live().await;
        live.drop_lanes(GapLanes::DURABLE).await;

        let key = Uuid::now_v7();
        live.publish(key, 1, Uuid::now_v7()).await;
        // v2 is never published.
        live.publish(key, 3, Uuid::now_v7()).await;

        let recorder = Recorder::new(Duration::ZERO, [key]);
        let running = tokio::spawn(run(
            live.js.clone(),
            Arc::new(GapLanes(recorder.clone())),
            live.db.clone(),
            live.readiness.clone(),
        ));
        until(Duration::from_secs(60), "the gap to give up", || {
            recorder.applies(key).len() == 2
        })
        .await;

        // The lane is still alive: a fourth event lands after the gap.
        live.publish(key, 4, Uuid::now_v7()).await;
        until(Duration::from_secs(30), "the event after the gap", || {
            recorder.applies(key).len() == 3
        })
        .await;
        running.abort();

        assert_eq!(recorder.versions(key), vec![1, 3, 4]);
        assert_eq!(
            live.row(key).await.map(|(_, version)| version),
            Some(4),
            "the gap must not hold the version back"
        );

        live.drop_lanes(GapLanes::DURABLE).await;
    }

    /// A rebuild bypasses the gate entirely.
    ///
    /// `outbox::backfill` re-emits an aggregate's whole chain — for a cancelled booking
    /// that is `Created`, `Confirmed`, `Cancelled` — with **every event carrying the same
    /// version**, because the chain exists to satisfy the downstream `WHERE status IN
    /// [...]` guards rather than to describe a version history.
    ///
    /// Gated, the second and third would be `Skip`ped as duplicates and a rebuilt
    /// projection would leave every cancelled booking sitting at `reserved`. Three
    /// applies at one version is what says the bypass is still there.
    #[tokio::test]
    #[ignore = "needs docker/docker-compose-dev.yml"]
    async fn a_backfill_chain_at_one_version_applies_every_step() {
        let live = live().await;
        live.drop_lanes(BackfillLanes::DURABLE).await;

        let key = Uuid::now_v7();
        for _ in 0..3 {
            live.publish_backfill(key, 7, Uuid::now_v7()).await;
        }

        let recorder = Recorder::new(Duration::ZERO, [key]);
        let running = tokio::spawn(run(
            live.js.clone(),
            Arc::new(BackfillLanes(recorder.clone())),
            live.db.clone(),
            live.readiness.clone(),
        ));
        until(Duration::from_secs(30), "the whole chain", || {
            recorder.applies(key).len() == 3
        })
        .await;
        running.abort();

        assert_eq!(
            recorder.versions(key),
            vec![7, 7, 7],
            "a gated backfill drops every step after the first"
        );

        live.drop_lanes(BackfillLanes::DURABLE).await;
    }
}
