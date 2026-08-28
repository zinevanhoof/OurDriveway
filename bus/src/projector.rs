use std::{sync::Arc, time::Duration};

use async_nats::jetstream::{
    Context,
    consumer::{AckPolicy, DeliverPolicy, PullConsumer},
};
use chrono::{DateTime, Utc};
use futures::StreamExt;
use shared::{
    error::myerror::{MyError, MyResult},
    events::{Envelope, PARTITIONS, domain_of, partition_filter},
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
    /// This replaced a `seq: u64` carrying the stream sequence. Its only consumer
    /// was booking-service storing a per-spot `bookings_seq` as a compare-and-swap
    /// precondition — a job `version` now does everywhere.
    fn apply(
        &self,
        tx: &Transaction<Client>,
        event: Self::Event,
        at: DateTime<Utc>,
        version: u64,
    ) -> impl Future<Output = MyResult<()>> + Send;
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
    /// `Arc`, so holding it is a refcount bump rather than a session. The session is
    /// minted per event by [`shared::db::begin`] and dropped with the transaction,
    /// which is what lets sixteen lanes share one socket: `Surreal::begin` consumes
    /// its client, so each lane needs a handle of its own, but only for as long as
    /// the transaction lasts.
    db: Arc<Surreal<Client>>,
}

impl<P> Tx<P> {
    /// Holds the connection rather than borrowing one from `projector`: this is the
    /// only place a transaction can be opened, so this is the only thing that needs
    /// one. A `Projector` therefore holds none at all and cannot reach the database
    /// except through the `&Transaction` it is handed.
    pub fn new(projector: Arc<P>, db: Arc<Surreal<Client>>) -> Self {
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
    async fn apply(&self, payload: &[u8], seq: u64) -> MyResult<()> {
        let envelope: Envelope<P::Event> = serde_json::from_slice(payload).map_err(|e| {
            shared::error::myerror::MyError::Bus(format!("decode {} at seq {seq}: {e}", P::STREAM))
        })?;
        let at = envelope.occurred_at;

        // A session of its own for this event, cloned off the shared connection and
        // dropped with the transaction.
        let tx = shared::db::begin(&self.db).await?;

        // Kept as a `Result` rather than an early `?`: `Transaction` is #[must_use]
        // and holds state on the server, so a dropped one keeps whatever it locked
        // until the server times it out.
        // The envelope's version, not the stream sequence: what the projector
        // stores has to be the number the owning service assigned and the client
        // is waiting on.
        let version = envelope.version;
        let applied = self
            .projector
            .apply(&tx, envelope.payload, at, version)
            .await;

        match applied {
            Ok(()) => {
                // The client `commit` hands back is dropped with its session; the
                // next event clones a fresh one.
                tx.commit().await?;
                Ok(())
            }
            Err(e) => {
                // Best effort, and no longer fatal to the lane: a rollback that
                // fails used to take this lane's only connection with it.
                tx.cancel().await.ok();
                Err(e)
            }
        }
    }
}

/// How long a message may be in flight before NATS assumes we died and
/// redelivers it. Must comfortably exceed the slowest `apply` — a redelivery
/// while the first attempt is still committing means two instances writing the
/// same rows, which TiKV refuses rather than corrupts, but which is still churn
/// nobody wants.
const ACK_WAIT: Duration = Duration::from_secs(30);

/// How often readiness is refreshed from the consumers' own state.
///
/// One second rather than the 250ms this was when there was a single consumer per
/// stream: one tick now costs `PARTITIONS` `consumer.info()` round trips, so the old
/// interval would have put ~64 requests a second on the broker per projector for a
/// number `/readyz` reads at human speed.
const POLL: Duration = Duration::from_secs(1);

/// One partition's durable name: `("view-users", 7)` -> `"view-users-p07"`.
///
/// Zero-padded so `nats consumer report` lists them in order rather than
/// `p0, p1, p10, p11, p2`. Hyphens because a durable name may not contain a `.`.
pub fn durable_name(prefix: &str, partition: u8) -> String {
    format!("{prefix}-p{partition:02}")
}

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
/// all of them would be N writers racing every row. The consumers are therefore
/// **durable and shared** — the replicas pull from one cursor per partition, so each
/// event is applied exactly once by whichever replica picked it up.
///
/// Work-sharing normally costs ordering, which this projection cannot afford:
/// downstream guards read `WHERE status IN $from` and *drop* a transition that
/// arrives before the state it expects. `max_ack_pending: 1` is what buys it back —
/// JetStream will not deliver the next message until the current one is acked.
///
/// That setting used to sit on **one** consumer per stream, which serialised the
/// whole stream: a booking for one spot blocked every unrelated user, spot and
/// payout behind it, and no number of replicas moved that. It now sits on one
/// consumer per partition, and NATS assigns the partition from the subject on
/// ingest (`shared::events::partition_transform`). Same aggregate → same subject →
/// same lane → still strictly ordered. Different aggregates → usually different
/// lanes → concurrent.
///
/// ## What this is not
///
/// Not an ownership protocol. Every replica opens all [`PARTITIONS`] lanes and a
/// partition's durable hands its one in-flight message to whichever puller asked
/// first, so partitions distribute themselves and a dead replica's lane is picked up
/// by another after `ACK_WAIT` with nothing to coordinate. Consumer pinning would add
/// affinity and, per Synadia's own writeup, still not exclusivity — and
/// `PriorityPolicy::PinnedClient` is unimplemented in async-nats 0.50 regardless.
///
/// The ceiling this buys is `PARTITIONS` concurrent applies **per stream, across the
/// whole deployment** — not per replica. Replicas buy availability; the partition
/// count buys throughput.
pub async fn run<P: Projector>(
    js: Context,
    projector: Arc<P>,
    db: Arc<Surreal<Client>>,
    readiness: Arc<Readiness>,
) {
    // The service's one connection, shared by every lane. Each event's transaction
    // runs on a session cloned off it and thrown away afterwards, so the lanes need
    // no connections of their own.
    //
    // Up front, so a projector that cannot declare its consumers fails here, once,
    // rather than sixteen times inside sixteen tasks.
    let consumers = match lane_consumers::<P>(&js).await {
        Ok(consumers) => consumers,
        Err(e) => {
            tracing::error!(stream = P::STREAM, durable = P::DURABLE, error = %e, "projector could not start");
            readiness.mark_failed(P::STREAM);
            return;
        }
    };

    // Readiness is reported from the consumers rather than from this instance's own
    // progress. With the events shared out between replicas, "how far have *I* got"
    // is not a fact about the database any more.
    tokio::spawn(report_readiness::<P>(consumers.clone(), readiness.clone()));

    tracing::info!(
        stream = P::STREAM,
        durable = P::DURABLE,
        lanes = PARTITIONS,
        "projecting"
    );

    let mut lanes = tokio::task::JoinSet::new();
    for (partition, consumer) in consumers.into_iter().enumerate() {
        // Every lane gets the same connection. `Surreal::begin` takes its handle by
        // value, so sixteen concurrent transactions still need sixteen handles —
        // they are just cloned per event and dropped with the transaction rather
        // than held open for the life of the process.
        let tx = Tx::new(projector.clone(), db.clone());
        lanes.spawn(async move {
            (partition as u8, lane::<P>(consumer, tx).await)
        });
    }

    // Deliberately does not abort the survivors. A wedged lane is one partition's
    // problem: its messages go unacked and another replica's lane picks them up,
    // while the other fifteen keep applying aggregates that have nothing to do with
    // it. `/readyz` is already 503, so this instance serves no reads either way —
    // stopping the healthy lanes too would only widen the outage.
    while let Some(joined) = lanes.join_next().await {
        match joined {
            Ok((partition, Err(e))) => {
                tracing::error!(stream = P::STREAM, durable = P::DURABLE, partition, error = %e, "lane stopped");
                readiness.mark_failed(P::STREAM);
            }
            Ok((partition, Ok(()))) => {
                tracing::error!(stream = P::STREAM, partition, "lane ended without error");
                readiness.mark_failed(P::STREAM);
            }
            Err(e) => {
                tracing::error!(stream = P::STREAM, error = %e, "lane panicked");
                readiness.mark_failed(P::STREAM);
            }
        }
    }
}

/// Declares one durable consumer per partition, in order.
///
/// Done up front rather than inside each lane so that [`report_readiness`] has every
/// consumer to poll from its first tick, and so a bad `P::STREAM` fails once at boot
/// instead of sixteen times in sixteen tasks.
async fn lane_consumers<P: Projector>(js: &Context) -> MyResult<Vec<PullConsumer>> {
    let domain = domain_of(P::STREAM)
        .ok_or_else(|| bus_err(format!("{} is not in shared::events::STREAMS", P::STREAM)))?;

    let stream = js
        .get_stream(P::STREAM)
        .await
        .map_err(|e| bus_err(format!("get stream {}: {e}", P::STREAM)))?;

    let mut consumers = Vec::with_capacity(PARTITIONS as usize);
    for partition in 0..PARTITIONS {
        let durable = durable_name(P::DURABLE, partition);
        consumers.push(
            stream
                .get_or_create_consumer(&durable, consumer_config(&durable, domain, partition))
                .await
                .map_err(|e| bus_err(format!("create consumer {durable}: {e}")))?,
        );
    }
    Ok(consumers)
}

/// One ordered lane. Applies and acks, strictly one message at a time.
///
/// Unchanged from the single-consumer loop this replaced, which is the point: the
/// ordering guarantee is the consumer's `max_ack_pending: 1`, so partitioning is a
/// matter of how many of these run rather than of what any one of them does.
async fn lane<P: Projector>(consumer: PullConsumer, projector: Tx<P>) -> MyResult<()> {
    let mut messages = consumer
        .messages()
        .await
        .map_err(|e| bus_err(format!("consume: {e}")))?;

    while let Some(msg) = messages.next().await {
        let msg = msg.map_err(|e| bus_err(format!("next message: {e}")))?;
        let seq = msg
            .info()
            .map_err(|e| bus_err(format!("message info: {e}")))?
            .stream_sequence;

        projector.apply(&msg.payload, seq).await?;

        // Only after the transaction committed. A crash in this gap leaves the
        // message unacked, so it is redelivered and reapplied — which is safe
        // precisely because every `apply` is idempotent, and is the reason there
        // is no longer a cursor to keep in step with the rows.
        msg.ack()
            .await
            .map_err(|e| bus_err(format!("ack {seq}: {e}")))?;
    }

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
async fn report_readiness<P: Projector>(consumers: Vec<PullConsumer>, readiness: Arc<Readiness>) {
    let mut consumers = consumers;
    loop {
        let mut all_drained = true;
        for consumer in &mut consumers {
            match consumer.info().await {
                // Not "sequence >= the head I saw at boot" — that number is this
                // instance's guess, and on a shared consumer it can be reached by
                // someone else's work or never reached at all if the head message
                // has since been deleted.
                Ok(info) => all_drained &= info.num_pending == 0 && info.num_ack_pending == 0,
                Err(e) => {
                    tracing::debug!(stream = P::STREAM, error = %e, "consumer info failed");
                    all_drained = false;
                }
            }
        }
        if all_drained {
            readiness.mark_caught_up(P::STREAM);
        }
        tokio::time::sleep(POLL).await;
    }
}

/// Split out so the settings that define this primitive can be asserted without a
/// broker. Four of the five are silent when wrong.
fn consumer_config(
    durable: &str,
    domain: &str,
    partition: u8,
) -> async_nats::jetstream::consumer::pull::Config {
    async_nats::jetstream::consumer::pull::Config {
        durable_name: Some(durable.to_string()),
        // This lane's partition and nothing else. NATS put the partition token there
        // on ingest; this is the only thing in the codebase that reads it, and it
        // reads it as a string.
        filter_subject: partition_filter(domain, partition),
        // Honoured only when the consumer is first created; afterwards the stored
        // position wins. `All` is what makes a brand-new projection build itself
        // from the whole log — the opposite of a `Worker`, which starts at `New`
        // precisely so deploying it does not re-send every email ever.
        deliver_policy: DeliverPolicy::All,
        ack_policy: AckPolicy::Explicit,
        ack_wait: ACK_WAIT,
        // The ordering guarantee, now scoped to one partition. Without it the
        // replicas pull concurrently and a `Confirmed` can be applied before its
        // `Created`, which the downstream `WHERE status IN $from` guards do not
        // reorder — they drop it.
        max_ack_pending: 1,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Guards the settings that make this safe on a shared database, all of which
    /// fail silently: without `max_ack_pending: 1` events apply out of order and
    /// transitions vanish; without a `durable_name` every replica gets every event
    /// and they race each other over every row; without `filter_subject` every lane
    /// consumes every partition, which is fan-out wearing a partition's name.
    #[test]
    fn the_consumer_is_durable_partitioned_and_strictly_ordered() {
        let durable = durable_name("view-bookings", 7);
        let config = consumer_config(&durable, "bookings", 7);

        assert_eq!(
            config.durable_name.as_deref(),
            Some("view-bookings-p07"),
            "an ephemeral consumer makes this fan-out: every replica applies every event"
        );
        assert_eq!(
            config.filter_subject, "bookings.7.>",
            "an unfiltered lane consumes every partition, so all sixteen would apply \
             every event and race each other over every row"
        );
        assert_eq!(
            config.max_ack_pending, 1,
            "more than one in flight lets a Confirmed overtake its Created, and the \
             downstream status guards drop it rather than reorder it"
        );
        assert!(
            matches!(config.deliver_policy, DeliverPolicy::All),
            "a projection must build from the whole log, unlike a Worker's DeliverPolicy::New"
        );
    }

    /// Two lanes must never claim the same message, and the set must cover the
    /// stream. Both hold only if the names and the filters agree partition for
    /// partition — a `-p7`/`bookings.8.>` pair would be silent in every other test.
    #[test]
    fn lanes_are_distinct_and_cover_the_stream() {
        let configs: Vec<_> = (0..PARTITIONS)
            .map(|p| consumer_config(&durable_name("view-bookings", p), "bookings", p))
            .collect();

        let mut durables: Vec<_> = configs
            .iter()
            .map(|c| c.durable_name.clone().expect("durable"))
            .collect();
        durables.sort();
        durables.dedup();
        assert_eq!(durables.len(), PARTITIONS as usize, "durable names collide");

        let mut filters: Vec<_> = configs.iter().map(|c| c.filter_subject.clone()).collect();
        filters.sort();
        filters.dedup();
        assert_eq!(filters.len(), PARTITIONS as usize, "filters collide");

        // Zero-padded name, unpadded subject token. They are not the same string and
        // it would be easy to "fix" that into a lane filtering `bookings.07.>`,
        // which matches nothing NATS ever writes.
        assert_eq!(configs[7].durable_name.as_deref(), Some("view-bookings-p07"));
        assert_eq!(configs[7].filter_subject, "bookings.7.>");
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
        collections::{HashMap, HashSet},
        sync::Mutex,
        time::Instant,
    };

    use async_nats::jetstream::message::PublishMessage;
    use shared::events::{Envelope, STREAM_SESSIONS, aggregate_id, session_subject};
    use uuid::Uuid;

    use super::*;

    const NATS: &str = "nats://127.0.0.1:4222";
    const ADDR: &str = "127.0.0.1:8000";
    /// Any database with a connection; the scratch table is created below.
    const DB: &str = "view";
    /// Written by these tests alone, and defined up front — sixteen lanes creating it
    /// implicitly would all write the same table-definition key and take a TiKV write
    /// conflict. An earlier spike learned that the hard way.
    const TABLE: &str = "_bus_livetest";

    /// How long a recorded apply holds its lane open.
    ///
    /// Long enough that two lanes running at once overlap observably, and that two
    /// events in one lane cannot appear to.
    const HOLD: Duration = Duration::from_millis(400);

    /// One applied event, with the window it occupied.
    #[derive(Clone, Debug)]
    struct Applied {
        key: Uuid,
        version: u64,
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

        fn versions(&self, key: Uuid) -> Vec<u64> {
            self.applies(key).iter().map(|a| a.version).collect()
        }

        async fn record(
            &self,
            tx: &Transaction<Client>,
            event: serde_json::Value,
            version: u64,
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

            // `version = version ?? 0` so the row starts at a number: `set_version`'s
            // `WHERE version < $v` compares against it, and NONE is not less than 1.
            // The projected tables get this from `DEFAULT 0` in their schema.
            tx.query(format!(
                "UPSERT type::record('{TABLE}', $i) SET
                     applies = (applies ?? 0) + 1,
                     version = (version ?? 0)"
            ))
            .bind(("i", key))
            .await?
            .check()?;

            shared::db::set_version(tx, TABLE, &key, version).await?;

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

    /// `Projector` carries its durable name as an associated const, so one test's
    /// lanes are one type. This mints them.
    macro_rules! recorder {
        ($name:ident, $durable:literal) => {
            struct $name(Arc<Recorder>);

            impl Projector for $name {
                const STREAM: &'static str = STREAM_SESSIONS;
                const DURABLE: &'static str = $durable;
                /// `Value`, not a real event enum: these lanes also see whatever else
                /// is on SESSIONS, and a decode failure stops a lane.
                type Event = serde_json::Value;

                async fn apply(
                    &self,
                    tx: &Transaction<Client>,
                    event: serde_json::Value,
                    _at: DateTime<Utc>,
                    version: u64,
                ) -> MyResult<()> {
                    self.0.record(tx, event, version).await
                }
            }
        };
    }

    recorder!(OrderLanes, "bus-lt-order");
    recorder!(ConcurrentLanes, "bus-lt-concurrent");
    recorder!(SerialLanes, "bus-lt-serial");
    recorder!(RedeliveryLanes, "bus-lt-redelivery");
    recorder!(FailoverLanes, "bus-lt-failover");
    recorder!(RestartLanes, "bus-lt-restart");
    recorder!(DuplicateLanes, "bus-lt-duplicate");
    recorder!(GapLanes, "bus-lt-gap");

    /// Defines the scratch table, tolerating the race between concurrent tests.
    ///
    /// `IF NOT EXISTS` is not a lock: two tests starting together both find it
    /// missing and both write the same table-definition key, which TiKV refuses as a
    /// write conflict — the exact hazard `bus/examples/tikv_spike` exists to
    /// demonstrate, arriving here as a flaky test. Retried rather than serialised,
    /// because the loser's retry finds the table already there and does nothing.
    async fn define_scratch_table(db: &Surreal<Client>) {
        for attempt in 0..5 {
            let result = db
                .query(format!("DEFINE TABLE IF NOT EXISTS {TABLE} SCHEMALESS"))
                .await
                .map_err(MyError::from)
                .and_then(|r| r.check().map_err(MyError::from));

            match result {
                Ok(_) => return,
                Err(e) if shared::db::is_write_conflict(&e) && attempt < 4 => {
                    tokio::time::sleep(Duration::from_millis(100 * (attempt + 1))).await;
                }
                Err(e) => panic!("scratch table: {e}"),
            }
        }
    }

    struct Live {
        js: Context,
        db: Arc<Surreal<Client>>,
        readiness: Arc<Readiness>,
    }

    async fn live() -> Live {
        shared::install_default_crypto_provider();

        let js = crate::connect(NATS)
            .await
            .expect("NATS on :4222 — docker compose -f docker/docker-compose-dev.yml up -d");
        crate::ensure_streams(&js).await.expect("declare streams");

        let db = shared::db::connect(ADDR, "root", "root", DB)
            .await
            .expect("SurrealDB on :8000 — same compose file");
        define_scratch_table(&db).await;

        let readiness = Readiness::new(js.client().clone(), &[STREAM_SESSIONS]);
        Live {
            js,
            db: Arc::new(db),
            readiness,
        }
    }

    impl Live {
        /// Publishes one event and returns the stream sequence NATS stored it at.
        ///
        /// Same shape as `outbox::append`: the envelope encoded, `Nats-Msg-Id` set to
        /// the event id so the stream's `duplicate_window` can see a repeat.
        async fn publish(&self, key: Uuid, version: u64, event_id: Uuid) -> u64 {
            let envelope = Envelope {
                event_id,
                aggregate: aggregate_id("session", &key),
                version,
                occurred_at: Utc::now(),
                actor_id: None,
                backfill: false,
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

        /// Which partition NATS put a stored message in.
        ///
        /// Read back off the stream rather than computed. Recomputing the hash in Rust
        /// is exactly what the server-side transform exists to avoid, and a test that
        /// reimplemented it would agree with itself while disagreeing with NATS.
        async fn partition_of(&self, sequence: u64) -> u8 {
            let stored = self
                .js
                .get_stream(STREAM_SESSIONS)
                .await
                .expect("stream")
                .get_raw_message(sequence)
                .await
                .expect("stored message")
                .subject;

            // `sessions.<partition>.user.<uuid>`
            stored
                .split('.')
                .nth(1)
                .and_then(|t| t.parse().ok())
                .unwrap_or_else(|| panic!("no partition token in stored subject {stored}"))
        }

        /// Publishes `n` fresh keys and returns them grouped by partition.
        async fn keys_by_partition(&self, n: usize) -> HashMap<u8, Vec<Uuid>> {
            let mut by_partition: HashMap<u8, Vec<Uuid>> = HashMap::new();
            for _ in 0..n {
                let key = Uuid::now_v7();
                let seq = self.publish(key, 1, Uuid::now_v7()).await;
                by_partition
                    .entry(self.partition_of(seq).await)
                    .or_default()
                    .push(key);
            }
            by_partition
        }

        /// Removes a test's lanes so `DeliverPolicy::All` is honoured on the next run.
        async fn drop_lanes(&self, prefix: &str) {
            let stream = self.js.get_stream(STREAM_SESSIONS).await.expect("stream");
            for partition in 0..PARTITIONS {
                let _ = stream.delete_consumer(&durable_name(prefix, partition)).await;
            }
        }

        /// `applies` and `version` as stored, for the idempotence assertions.
        async fn row(&self, key: Uuid) -> Option<(i64, i64)> {
            let applies: Option<i64> = self
                .db
                .query(format!("SELECT VALUE applies FROM ONLY type::record('{TABLE}', $i)"))
                .bind(("i", key))
                .await
                .ok()?
                .take(0)
                .ok()?;
            let version: Option<i64> = self
                .db
                .query(format!("SELECT VALUE version FROM ONLY type::record('{TABLE}', $i)"))
                .bind(("i", key))
                .await
                .ok()?
                .take(0)
                .ok()?;
            Some((applies?, version?))
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

        assert_eq!(recorder.versions(key), vec![1, 2, 3], "applied out of order");

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

    /// Requirement 2. Aggregates in different partitions are applied at the same
    /// time — which is the entire point of the change, and was impossible before it.
    #[tokio::test]
    #[ignore = "needs docker/docker-compose-dev.yml"]
    async fn different_keys_run_concurrently() {
        let live = live().await;
        live.drop_lanes(ConcurrentLanes::DURABLE).await;

        // Publish first, then look up where NATS put them: with 16 partitions a
        // dozen keys land in several, and this asserts against whichever two the
        // server actually separated.
        let by_partition = live.keys_by_partition(12).await;
        let mut occupied: Vec<_> = by_partition.iter().filter(|(_, k)| !k.is_empty()).collect();
        occupied.sort_by_key(|(p, _)| **p);
        assert!(
            occupied.len() >= 2,
            "twelve keys landed in one partition — that is a broken transform, not luck"
        );
        let (left, right) = (occupied[0].1[0], occupied[1].1[0]);

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
            "partitions {} and {} ran one after the other, so nothing was parallelised",
            occupied[0].0,
            occupied[1].0
        );

        live.drop_lanes(ConcurrentLanes::DURABLE).await;
    }

    /// Two aggregates that share a partition are still applied, and still one at a
    /// time — the cost of a bounded partition count, asserted rather than assumed.
    #[tokio::test]
    #[ignore = "needs docker/docker-compose-dev.yml"]
    async fn one_partition_stays_serial() {
        let live = live().await;
        live.drop_lanes(SerialLanes::DURABLE).await;

        // 16 partitions, so a collision inside 24 keys is near-certain — but not
        // guaranteed, and a test must not assert on luck.
        let by_partition = live.keys_by_partition(24).await;
        let Some((partition, keys)) = by_partition.iter().find(|(_, k)| k.len() >= 2) else {
            eprintln!("no two of 24 keys shared a partition; nothing to assert");
            live.drop_lanes(SerialLanes::DURABLE).await;
            return;
        };
        let (first, second) = (keys[0], keys[1]);

        let recorder = Recorder::new(HOLD, [first, second]);
        let running = tokio::spawn(run(
            live.js.clone(),
            Arc::new(SerialLanes(recorder.clone())),
            live.db.clone(),
            live.readiness.clone(),
        ));

        until(Duration::from_secs(60), "both keys applied", || {
            !recorder.applies(first).is_empty() && !recorder.applies(second).is_empty()
        })
        .await;
        running.abort();

        let (a, b) = (
            recorder.applies(first).remove(0),
            recorder.applies(second).remove(0),
        );
        assert!(
            !a.overlaps(&b),
            "partition {partition} had two aggregates in flight at once — \
             max_ack_pending is not holding"
        );

        live.drop_lanes(SerialLanes::DURABLE).await;
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

    /// Requirements 3 and 4. Two instances share the lanes; killing the one holding a
    /// message hands it to the other with nothing to coordinate.
    ///
    /// Slow by construction — the handover is `ACK_WAIT`, which is what makes it a
    /// failover rather than a graceful drain.
    #[tokio::test]
    #[ignore = "needs docker/docker-compose-dev.yml; takes ~ack_wait"]
    async fn a_dead_instance_hands_its_partition_over() {
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
        until(Duration::from_secs(60), "the message to be in flight", || {
            doomed.entered() > 0
        })
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

        // Same id, well inside the stream's 120s duplicate_window.
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

    /// Requirement 7, the other half. A missing version is applied — but explicitly,
    /// and the lane survives it.
    ///
    /// That it is *reported* is `shared::db::version_gap`'s unit test; that it does
    /// not wedge the projection is this one. Both halves matter: a gap that stopped
    /// the lane would turn an aged-out history into an outage.
    #[tokio::test]
    #[ignore = "needs docker/docker-compose-dev.yml"]
    async fn a_version_gap_is_applied_and_does_not_wedge_the_lane() {
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
        until(Duration::from_secs(30), "both events", || {
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
}
