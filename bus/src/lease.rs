//! A named, expiring lease held by exactly one instance of a service.
//!
//! Every replica of a service shares one database, and **the outbox relay** must not
//! run on more than one of them at a time: ordering within a service's outbox is what
//! keeps a booking's `reserved → confirmed → cancelled` sequence intact, and two
//! relays interleaving can publish those out of order — which the downstream
//! `WHERE status IN …` guards drop rather than reorder.
//!
//! The projectors do not take a lease. Each partition is one durable consumer with
//! `max_ack_pending: 1`, so JetStream hands out one event at a time per partition
//! across every replica, in order.
//!
//! ## What this is, and is not, protecting
//!
//! Worth being precise about, because leases invite more trust than they deserve —
//! and because the answer **changed with the database**.
//!
//! Under TiKV this was an optimisation with a hard net underneath: two relays
//! deleting one outbox row was a write-write conflict, so the store refused one of
//! them and the transaction prevented the corruption. Read Committed does not refuse
//! it. The second relay blocks, re-reads, and its `DELETE` simply affects zero rows.
//!
//! So what actually stands behind a lapsed lease now is thinner, and it is worth
//! knowing exactly what:
//!
//!   - **Duplicate publishes** are absorbed by `Nats-Msg-Id` against the stream's
//!     120s `duplicate_window` — outside that window the event genuinely lands twice,
//!     and idempotent UPSERT projectors are what absorb it.
//!   - **Ordering has no net at all**, and never did. Two relays publishing one
//!     aggregate's events concurrently can land v2 after v3, where `set_version`'s
//!     `WHERE version < $v` holds the version but not the columns, so the row reads
//!     stale until that aggregate's next real event.
//!
//! Which makes the lease **load-bearing rather than an optimisation**. `drain`
//! re-checks it between batches for exactly this reason; see the note there.
//!
//! ## Clock
//!
//! Expiry is evaluated with `now()` **inside the database**, never against a caller's
//! clock. Replicas do not share a clock; they do share this row.

use std::time::Duration;

use diesel::prelude::*;
use diesel::sql_types::{Double, Timestamptz};
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use shared::db::Db;
use shared::error::myerror::MyResult;

use crate::schema::_lease;

/// How long a lease stays valid after the last successful renewal.
///
/// The trade is failover latency against tolerance for a slow tick: a holder that
/// misses this window loses the lease, and nothing else can take over until it
/// lapses. Comfortably longer than [`RENEW_EVERY`] so an ordinary hiccup does not
/// cause a handover.
pub const TTL: Duration = Duration::from_secs(30);

/// How often the holder renews. A third of [`TTL`], so two consecutive failures
/// are survivable.
pub const RENEW_EVERY: Duration = Duration::from_secs(10);

/// The one lease per service.
///
/// Deliberately not one per stream, nor one for the projectors and another for the
/// relay. Everything that needs a single runner needs it for the same reason —
/// they all write this service's one database — and splitting the lease would only
/// let two instances each hold half the work, which buys nothing and doubles the
/// ways a handover can go wrong.
pub const LEADER: &str = "leader";

/// Takes `name` if it is free or expired, renews it if we already hold it, and
/// leaves it alone if someone else does.
///
/// Returns whether we hold the lease when this returns. `false` is the ordinary
/// answer on a non-leader replica, not an error.
///
/// **One statement**, which is what makes it safe. The condition and the write are
/// evaluated together, so there is no window between deciding the lease is free and
/// taking it.
///
/// `INSERT … ON CONFLICT DO UPDATE … WHERE` is the direct translation of the
/// `UPSERT … WHERE` this replaced, and the subtle clause is the same one: the `WHERE`
/// guards only the *update* branch, so a lease nobody holds is inserted regardless.
/// That is exactly the wanted behaviour, and it is the one part here whose semantics
/// are not obvious from reading it.
///
/// `RETURNING` yields a row only when the insert or the update actually happened, so
/// an empty result means someone else holds it — no second read, and no window
/// between the two.
///
/// Two instances racing a free lease both try to insert the same primary key. One
/// wins; the loser's `ON CONFLICT` branch then evaluates the `WHERE` against the
/// winner's fresh row and declines. Under Read Committed the loser blocks until the
/// winner commits and then sees the committed row, which is precisely the behaviour
/// this needs — see `shared::db` for why that is stated rather than assumed.
///
/// An earlier SurrealDB version wrapped a `SELECT` and an `IF` in explicit
/// `BEGIN`/`COMMIT`. It worked over HTTP and silently returned `false` forever
/// through the Rust client, whose statement indexing did not line up the same way
/// across a transaction block. One statement has no index to get wrong.
pub async fn acquire(conn: &mut AsyncPgConnection, name: &str, holder: &str) -> MyResult<bool> {
    // Imported HERE and not at module scope. `QueryDsl::filter` covers SELECTs; the
    // `WHERE` on a DO UPDATE branch comes from `FilterDsl` implemented directly on
    // `InsertStatement<_, OnConflictValues<..>>`. Both are in scope at module level and
    // every ordinary `.filter()` in this file then becomes ambiguous.
    use diesel::query_dsl::methods::FilterDsl;

    // `make_interval(secs => …)` rather than formatting a string and casting it: the
    // TTL is a Duration, and turning it into "30s" for the database to parse back is
    // a round trip through text that can only go wrong. It has no DSL spelling, so it
    // is a typed `sql::<Timestamptz>` fragment with the seconds bound.
    let expires_at = diesel::dsl::sql::<Timestamptz>("now() + make_interval(secs => ")
        .bind::<Double, _>(TTL.as_secs() as f64)
        .sql(")");

    // The `WHERE` on the DO UPDATE branch is the whole mechanism: without it the
    // conflicting insert would steal a live lease. `.filter()` on the insert statement
    // is what emits it — diesel implements `FilterDsl` for an `InsertStatement` whose
    // values carry an `OnConflictValues`, so this is the conflict clause and not an
    // ordinary predicate.
    let held: Vec<String> = diesel::insert_into(_lease::table)
        .values((
            _lease::name.eq(name),
            _lease::holder.eq(holder),
            _lease::expires_at.eq(expires_at.clone()),
        ))
        .on_conflict(_lease::name)
        .do_update()
        .set((_lease::holder.eq(holder), _lease::expires_at.eq(expires_at)))
        .filter(
            _lease::holder
                .eq(holder)
                .or(_lease::expires_at.lt(diesel::dsl::now)),
        )
        .returning(_lease::holder)
        .load(conn)
        .await?;

    Ok(!held.is_empty())
}

/// Gives the lease up immediately rather than waiting out [`TTL`].
///
/// Best effort, and deliberately scoped to our own holding: a losing racer must
/// not be able to delete the winner's lease. Called on clean shutdown so a rolling
/// restart hands over in milliseconds instead of half a minute.
pub async fn release(conn: &mut AsyncPgConnection, name: &str, holder: &str) -> MyResult<()> {
    diesel::delete(
        _lease::table
            .filter(_lease::name.eq(name))
            .filter(_lease::holder.eq(holder)),
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// Spawns the election, and returns a handle every task can watch.
///
/// Call once per service, at boot. The returned receiver is `true` for as long as
/// this instance is the leader; projectors and the outbox relay clone it and start
/// or stop their work as it flips. One task, one connection, one query every
/// [`RENEW_EVERY`] — however many things end up watching it.
///
/// A losing replica is **healthy, not degraded**: it serves requests off the same
/// database the leader is keeping current. Nothing here should be wired to
/// readiness.
///
/// Takes the pool by value — it is `Arc` inside, so this is a refcount bump and the
/// spawned task borrows a connection for the length of one statement every
/// [`RENEW_EVERY`] and gives it straight back.
///
/// The long paragraph that used to be here — about whether this loop needed a
/// *separate connection* because a projector's open transaction might block every
/// other session on the shared socket — is gone with the socket. That was a real
/// hazard when one WebSocket carried every session; a pool has no such coupling.
pub fn elect(db: Db, holder: String) -> tokio::sync::watch::Receiver<bool> {
    let (tx, rx) = tokio::sync::watch::channel(false);

    tokio::spawn(async move {
        loop {
            // A pool checkout per attempt, every ten seconds. Cheap, and it means a
            // connection is not held idle across the sleep.
            let attempt = match db.get().await {
                Ok(mut conn) => acquire(&mut conn, LEADER, &holder).await,
                Err(e) => Err(shared::error::myerror::MyError::Pool(e.to_string())),
            };

            let held = match attempt {
                Ok(held) => {
                    tracing::debug!(holder, held, "lease attempt");
                    held
                }
                // Losing the write race against another instance lands here, and
                // is the mechanism working rather than a fault. Treated as "not
                // the leader" and retried.
                Err(e) => {
                    tracing::debug!(error = %e, "lease attempt failed");
                    false
                }
            };

            // `send_if_modified` so watchers wake on a transition and not on every
            // renewal — a projector does not want a wakeup every ten seconds.
            tx.send_if_modified(|was| {
                if *was != held {
                    match held {
                        true => tracing::info!(holder, "became leader"),
                        false => tracing::warn!(holder, "lost leadership"),
                    }
                    *was = held;
                    true
                } else {
                    false
                }
            });

            tokio::time::sleep(RENEW_EVERY).await;
        }
    });

    rx
}

/// A stable-enough identity for one running process.
///
/// Hostname would be stable across a pod restart and is exactly what we do *not*
/// want: a restarted pod must not be able to renew the lease its previous
/// incarnation held. A fresh uuid per process is the right grain.
pub fn instance_id() -> String {
    uuid::Uuid::now_v7().to_string()
}
