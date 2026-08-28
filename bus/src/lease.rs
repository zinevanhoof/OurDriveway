//! A named, expiring lease held by exactly one instance of a service.
//!
//! Every replica of a service now shares one database (TiKV is authoritative), and
//! two things in this crate must not run on more than one of them at a time:
//!
//!   - **projectors**, because two of them race the `_projection` cursor row and
//!     because work-sharing would reorder events past guards like
//!     `WHERE status IN $from`;
//!   - **the outbox relay**, because ordering within a service's outbox is what
//!     keeps a booking's `reserved → confirmed → cancelled` sequence intact.
//!
//! Both are "exactly one runner per service", so both take a lease from here.
//!
//! ## This is not the safety mechanism
//!
//! Worth being clear about, because leases invite more trust than they deserve. A
//! holder that stalls past its expiry while another instance takes over would, in
//! most systems, mean two writers. Here it does not, because the writes those two
//! runners make collide in TiKV: two projectors bumping one cursor row, or two
//! relays deleting one outbox row, is a write-write conflict and one of them is
//! refused.
//!
//! So **the transaction is what prevents corruption; the lease only prevents
//! thrash.** Without it, N replicas would each apply every event and N-1 would
//! abort on every single one — correct, and unusable. Read the lease as an
//! optimisation with a safety net underneath it, not as mutual exclusion.
//!
//! ## Clock
//!
//! Expiry is evaluated with `time::now()` **inside the database**, never against a
//! caller's clock. Replicas do not share a clock; they do share this database.

use std::{sync::Arc, time::Duration};

use shared::{db::Querier, error::myerror::MyResult};
use surrealdb::{Surreal, engine::remote::ws::Client};

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
/// evaluated together, so there is no window between deciding the lease is free
/// and taking it. Two instances racing a free lease both write the same key, and
/// TiKV refuses one of them — that refusal surfaces here as `Err`, which callers
/// treat as "not the leader".
///
/// The `WHERE` does not apply to the create case: on a missing row `UPSERT`
/// inserts regardless, which is exactly the wanted behaviour for a lease nobody
/// holds. Verified against SurrealDB 3.2.4 rather than assumed — it is the one
/// clause here whose semantics are not obvious from reading it.
///
/// An earlier version wrapped a `SELECT` and an `IF` in explicit `BEGIN`/`COMMIT`.
/// It worked over HTTP `/sql` and silently returned `false` forever through the
/// Rust client, whose statement indexing does not line up the same way across a
/// transaction block. One statement has no index to get wrong.
pub async fn acquire(db: &impl Querier, name: &str, holder: &str) -> MyResult<bool> {
    // Non-empty means the write applied and the lease is ours. Only the count
    // matters, so the row is deserialized as its holder rather than as a struct
    // this function would otherwise have to define and keep in step.
    let held: Vec<String> = db
        .q("UPSERT type::record('_lease', $name)
                SET holder = $holder, expires_at = time::now() + type::duration($ttl)
                WHERE holder = $holder OR expires_at < time::now()
                RETURN VALUE holder")
        .bind(("name", name.to_string()))
        .bind(("holder", holder.to_string()))
        .bind(("ttl", format!("{}s", TTL.as_secs())))
        .await?
        .take(0)?;

    Ok(!held.is_empty())
}

/// Gives the lease up immediately rather than waiting out [`TTL`].
///
/// Best effort, and deliberately scoped to our own holding: a losing racer must
/// not be able to delete the winner's lease. Called on clean shutdown so a rolling
/// restart hands over in milliseconds instead of half a minute.
pub async fn release(db: &impl Querier, name: &str, holder: &str) -> MyResult<()> {
    db.q("DELETE type::record('_lease', $name) WHERE holder = $holder")
        .bind(("name", name.to_string()))
        .bind(("holder", holder.to_string()))
        .await?
        .check()?;
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
/// Shares the service's one connection. `acquire` is a single `UPSERT` — it never
/// opens a transaction — so it needs no handle of its own.
///
/// This used to demand a separate connection, on the theory that a projector's open
/// transaction blocks every other session on the socket and this loop then hangs
/// inside `acquire`, silently, so the service never elects. Measured directly, that
/// does not reproduce: with 64 transactions open on one socket, a statement on another
/// session of it returns in ~5ms. The hang was the clone's own sign-in, not the socket
/// — see `bus/examples/clone_cost`.
pub fn elect(db: Arc<Surreal<Client>>, holder: String) -> tokio::sync::watch::Receiver<bool> {
    let (tx, rx) = tokio::sync::watch::channel(false);

    tokio::spawn(async move {
        loop {
            let held = match acquire(&db, LEADER, &holder).await {
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
