//! Transactional outbox: the bridge between a database commit and a NATS append.
//!
//! ## What this is for, and what it is not for
//!
//! JetStream already provides most of what an outbox is usually built to add.
//! Publishes carry `Nats-Msg-Id` (the envelope's `event_id`) and the streams set a
//! `duplicate_window`, so a retried publish is discarded server-side. Consumers are
//! durable and at-least-once, and every projector applies with an idempotent
//! UPSERT. None of that is reimplemented here.
//!
//! The one thing no broker feature can provide is **atomicity between the database
//! commit and the append**. JetStream's guarantees begin once a message is in the
//! stream; the window between "my transaction committed" and "the stream accepted
//! it" is invisible to it. Today that window does not exist, because the append
//! *is* the commit. The moment TiKV becomes authoritative it does, and this closes
//! it: the event is written as a row in the same transaction as the data, and a
//! relay moves it to NATS afterwards.
//!
//! ## Delivery
//!
//! At-least-once, deliberately. The relay publishes, then deletes the row; a crash
//! between those two republishes on restart. Inside the stream's 120s
//! `duplicate_window` that is discarded server-side. **Outside** it — a relay down
//! for longer than two minutes — the event genuinely lands twice, and what absorbs
//! it is the consumer side: idempotent UPSERT projectors, and workers that already
//! carry provider idempotency keys. The dedupe window and the consumers cover
//! different durations; do not read either as covering both.
//!
//! ## Ordering
//!
//! One relay per service, via [`crate::lease`]. Without it, two relays interleave
//! and a booking's `reserved → confirmed → cancelled` can reach a consumer out of
//! order — which matters, because the projector guards transitions with
//! `WHERE status IN $from` and simply drops one that does not match.

use std::time::Duration;

use async_nats::jetstream::{Context, message::PublishMessage};
use chrono::{DateTime, Utc};
use diesel::prelude::*;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use serde::Serialize;
use shared::db::Db;
use shared::{
    error::myerror::{MyError, MyResult},
    events::Envelope,
};

use crate::schema::_outbox;
use tokio::sync::watch;
use uuid::Uuid;

/// How long the relay sleeps when it finds nothing to send.
///
/// Not a latency floor for the common case: [`drain`] loops until the table is
/// empty, so a burst is sent back-to-back and only the tail waits this long.
const IDLE: Duration = Duration::from_millis(200);

/// Rows per pass. Bounds how much one iteration holds in memory and how much a
/// crash mid-pass has to redo.
const BATCH: usize = 128;

/// One pending event. The envelope is stored encoded, exactly as it will be sent.
#[derive(Debug, Queryable, Selectable)]
#[diesel(table_name = _outbox)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct Pending {
    pub id: Uuid,
    pub subject: String,
    /// The serialized [`Envelope`]. Text rather than bytes so a stuck row can be
    /// read by a human with a `SELECT`, which is the whole reason this table is
    /// worth looking at when something has gone wrong.
    pub payload: String,
    /// **Relay order, not business time.** Written by the database's clock, not the
    /// enqueuing replica's — see [`enqueue`]. The envelope's `occurred_at` is the
    /// business timestamp and is inside `payload`.
    pub created_at: DateTime<Utc>,
}

/// Writes an event into the caller's **open transaction**.
///
/// Callers pass their open transaction as `&mut *tx` rather than a pooled connection,
/// because being in the caller's transaction is the entire point: if their write rolls
/// back, so does this, and no event is ever published for a change that did not
/// happen. The signature is `impl PgExecutor<'_>`, which admits both — `backfill`
/// below is the one caller that legitimately passes the pool.
///
/// The record id is the envelope's `event_id`, so enqueuing the same event twice
/// is one row rather than two — the same property the `Nats-Msg-Id` dedupe gives
/// on the way out, applied on the way in.
///
/// ## `created_at` is `time::now()`, not `envelope.occurred_at`
///
/// [`drain`] orders by this column, so it is what decides publication order — and
/// per-aggregate publication order is what the partitioned consumers downstream can
/// preserve but cannot repair.
///
/// It used to be `occurred_at`, which is `Utc::now()` on the **enqueuing replica**.
/// Two replicas do not share a clock. A replica running 50ms fast could stamp an
/// aggregate's v3 with an earlier time than another replica stamped its v2, and the
/// relay would then publish them in that order — v2 landing after v3, where
/// `set_version`'s `WHERE version < $v` holds the version but not the columns, so the
/// row reads stale until its next real event. The `id` tiebreak does not help: a
/// UUIDv7 comes off the same skewed clock.
///
/// `now()` is evaluated inside the database, which every replica shares — the same
/// reason `lease::acquire` evaluates expiry there rather than against a caller's
/// clock. (Transaction-start time, not `clock_timestamp()`: two events enqueued in one
/// transaction then share a timestamp, which is exactly why [`drain`] breaks the tie
/// on `id`.)
///
/// This also separates two things that were only ever coincidentally equal:
/// `occurred_at` is when the thing happened and stays in the envelope for projectors
/// to read, `created_at` is where the row sits in the relay queue.
pub async fn enqueue<T: Serialize>(
    conn: &mut AsyncPgConnection,
    subject: &str,
    envelope: &Envelope<T>,
) -> MyResult<()> {
    let payload = serde_json::to_string(envelope)
        .map_err(|e| MyError::Bus(format!("serialize outbox event: {e}")))?;

    // `created_at` is left to the column's own `now()` default rather than set here, so
    // it stays the DATABASE's transaction-start time. Replicas do not share a clock and
    // this value is the relay's ordering key.
    diesel::insert_into(_outbox::table)
        .values((
            _outbox::id.eq(envelope.event_id),
            _outbox::subject.eq(subject),
            _outbox::payload.eq(&payload),
        ))
        .on_conflict(_outbox::id)
        .do_update()
        .set((_outbox::subject.eq(subject), _outbox::payload.eq(&payload)))
        .execute(conn)
        .await?;
    Ok(())
}

/// Re-emits one aggregate's current state as the events that reproduce it.
///
/// This is the rebuild path, and it exists because the streams expire now
/// (`shared::events::STREAMS`). Replaying history is no longer possible and no
/// longer needed — TiKV is durable — but a projection can still be *wrong*: a
/// projector bug, a new denormalized column, a restore from a stale backup. When
/// that happens, this walks what the owning service actually has and enqueues the
/// events for it, so the re-derive runs through the same projectors as live traffic
/// instead of a one-off script that duplicates their denormalization rules.
///
/// Takes the pool rather than a transaction, unlike [`enqueue`]: there is no
/// accompanying write to be atomic with, and a whole-table backfill in one transaction
/// would be a write set the size of the table. Each row is its own statement, and a
/// run that dies half way is resumed by running it again.
///
/// Event ids are derived from `<aggregate>@<version>:<step>`, so a second run writes
/// the same `_outbox` rows and — inside the stream's `duplicate_window` — publishes
/// once. `backfill` is set on every envelope; see [`Envelope::backfill`] for the one
/// consumer that must honour it.
///
/// `events` carries its own timestamps because a chain is not simultaneous: a spot's
/// `Created` must land with the row's `created_at`, or view-service's copy claims the
/// listing appeared the day the backfill ran.
///
/// ponytail: safe against a projection that is behind or empty, which is the case it
/// is for. Run against one that is *ahead* — or racing a live write to the same
/// aggregate — and the older content can land on top of newer: `set_version`'s
/// `WHERE version < $v` holds the version back but not the columns, so the row reads
/// stale until its next real event. Compare versions inside each projector if this
/// ever needs to be safe concurrently.
pub async fn backfill<T: Serialize>(
    pool: &Db,
    subject: &str,
    aggregate: &str,
    version: i64,
    events: impl IntoIterator<Item = (DateTime<Utc>, T)>,
) -> MyResult<usize> {
    let mut sent = 0;
    for (step, (occurred_at, payload)) in events.into_iter().enumerate() {
        let envelope = Envelope {
            event_id: Uuid::new_v5(
                &Uuid::NAMESPACE_OID,
                format!("backfill:{aggregate}@{version}:{step}").as_bytes(),
            ),
            aggregate: aggregate.to_string(),
            version,
            occurred_at,
            // Nobody caused this. The original actor is not recoverable from a row,
            // and inventing one would put a lie in the log.
            actor_id: None,
            backfill: true,
            payload,
        };
        // `backfill` holds the pool rather than a transaction — each row is its own
        // statement and a partial backfill is safe to resume — so it checks out a
        // connection per event. `enqueue` itself takes a connection precisely so its
        // OTHER callers can hand it an open transaction.
        let mut conn = pool.get().await.map_err(|e| MyError::Pool(e.to_string()))?;
        enqueue(&mut conn, subject, &envelope).await?;
        sent += 1;
    }
    Ok(sent)
}

/// Publishes everything pending, oldest first, until the table is empty.
///
/// Returns how many were sent. Stops at the first failure and leaves the rest —
/// the row is still there, so the next pass retries it, and stopping preserves
/// order rather than skipping past a subject that is refusing writes.
///
/// `leader` is re-checked between batches, and that is not belt-and-braces. This
/// loops until the table is empty, which can be a long time behind a backlog, while
/// the outer [`run`] only checks leadership between calls. A relay that stalled past
/// the lease TTL, lost it, and then resumed would otherwise keep publishing a batch
/// it read minutes ago — interleaving with the new leader's fresh one. Inside the
/// stream's 120s `duplicate_window` the stale copies are discarded server-side and
/// order survives; beyond it a stale v2 lands after v3 and the projection reads stale
/// until that aggregate's next event.
///
/// This does not close the window entirely — a stall *inside* a single publish still
/// gets one message out — but one message is bounded and a whole backlog is not.
pub async fn drain(pool: &Db, js: &Context, leader: &watch::Receiver<bool>) -> MyResult<usize> {
    let mut sent = 0;
    loop {
        if !*leader.borrow() {
            return Ok(sent);
        }

        // `created_at` then `id`: two events enqueued in the same transaction share a
        // timestamp (`now()` is transaction-start time), and the id breaks the tie
        // deterministically so a retry after a crash sends them in the same order as
        // the first attempt.
        //
        // Deliberately NOT `FOR UPDATE SKIP LOCKED`, which is the reflex for a queue
        // table and would be wrong here: skipping locked rows lets a second relay take
        // the *next* batch and publish it first, which is precisely the reordering the
        // lease exists to prevent. Order matters more than throughput on this table.
        let mut conn = pool.get().await.map_err(|e| MyError::Pool(e.to_string()))?;

        let batch: Vec<Pending> = _outbox::table
            .order((_outbox::created_at.asc(), _outbox::id.asc()))
            .limit(BATCH as i64)
            .select(Pending::as_select())
            .load(&mut *conn)
            .await?;

        if batch.is_empty() {
            return Ok(sent);
        }

        for row in batch {
            append(js, row.subject.clone(), &row.id.to_string(), row.payload).await?;

            // Only after the ack. A crash in this gap republishes on restart, which is
            // what makes this at-least-once rather than at-most-once — the safer side
            // to be wrong on, given the consumers are idempotent.
            diesel::delete(_outbox::table.filter(_outbox::id.eq(row.id)))
                .execute(&mut *conn)
                .await?;

            sent += 1;
        }
    }
}

/// Runs the relay for as long as this instance is the leader.
///
/// Spawn one per service, beside the projectors, sharing the same
/// [`crate::lease::elect`] handle — one election decides both, because both are "one
/// runner per service" for the same reason.
///
/// A follower parks on `leader.changed()` and costs nothing at all; the election
/// task is already paying the one query every ten seconds.
pub async fn run(pool: Db, js: Context, mut leader: watch::Receiver<bool>) {
    loop {
        while !*leader.borrow() {
            if leader.changed().await.is_err() {
                return; // election task gone; so is the process
            }
        }

        tracing::info!("outbox relay started");
        while *leader.borrow() {
            match drain(&pool, &js, &leader).await {
                Ok(0) => tokio::time::sleep(IDLE).await,
                Ok(n) => tracing::debug!(count = n, "relayed outbox events"),
                Err(e) => {
                    // Never fatal. The rows are still there, so the next pass
                    // retries; a relay that gave up would strand every event the
                    // service has committed since.
                    tracing::error!(error = %e, "outbox drain failed; retrying");
                    tokio::time::sleep(IDLE).await;
                }
            }
        }
        tracing::info!("outbox relay stopped; no longer leader");
    }
}

/// Appends one already-encoded envelope to its stream.
///
/// **The only place anything in this codebase writes to NATS.** It used to be one
/// of five — `publish`, `publish_expecting`, `subject_head`, `catch_up` and this —
/// in a `publisher` module that no longer had a reason to exist once services
/// stopped publishing directly. The compare-and-swap half went with the per-spot
/// subject CAS; a database transaction serialises a booking now.
///
/// `message_id` is the envelope's `event_id`, which the stream's `duplicate_window`
/// uses to discard a republish — the relay is at-least-once, so this matters.
async fn append(js: &Context, subject: String, event_id: &str, payload: String) -> MyResult<u64> {
    let ack = js
        .send_publish(
            subject,
            PublishMessage::build()
                .message_id(event_id)
                .payload(payload.into_bytes().into()),
        )
        .await
        .map_err(|e| MyError::Bus(format!("publish: {e}")))?
        .await;

    ack.map(|ack| ack.sequence)
        .map_err(|e| MyError::Bus(format!("publish ack: {e}")))
}
