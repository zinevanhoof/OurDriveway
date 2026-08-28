use std::borrow::Cow;

use uuid::Uuid;

use surrealdb::{
    Surreal,
    engine::remote::ws::{Client, Ws},
    method::{Query, Transaction},
    opt::auth::Root,
};

use crate::error::myerror::{MyError, MyResult};

/// Opens the service's own connection to its own database.
///
/// One connection per process, authenticated once at boot with a database-level
/// user. This replaced the old per-request `db.authenticate(client_jwt)` in
/// `AuthedDb`, where every request re-authenticated the single shared socket —
/// so two concurrent requests could each be running under the other's identity.
///
/// Because this identity has a role rather than a record, table-level
/// `PERMISSIONS` do not constrain it. Authorization is the service's job now:
/// verify the JWT with `AuthedJwt`, then check ownership in Rust.
///
/// Credentials come from the caller's `Config`, not from this crate reading the
/// environment behind its back — the old `SURREALDB_USER` default of "root" was
/// a value no `.env` file mentioned.
/// `database` is the service's own database inside the shared `main` namespace —
/// "user", "booking", "spot", "payment", "view". It used to be "main" for every
/// service, which was safe only because each ran its own SurrealDB process over
/// its own datastore. On TiKV they share one keyspace, so the database name is
/// now the only thing separating one service's tables from another's.
pub async fn connect(
    addr: &str,
    username: &str,
    password: &str,
    database: &str,
) -> MyResult<Surreal<Client>> {
    let db = Surreal::new::<Ws>(addr).await?;

    db.signin(Root {
        username: username.to_string(),
        password: password.to_string(),
    })
    .await?;

    db.use_ns("main").use_db(database).await?;

    Ok(db)
}

// A note on `warm()` — a `Surreal::clone` followed by a polled `RETURN 1`, which used
// to live here so that a lane could hold a pre-authenticated session.
//
// It does not work, and the reason is worth keeping because the wrong diagnosis was
// expensive. `clone` mints a session id and asks the engine to *replay*
// `Attach`/`Signin`/`Use` onto it without awaiting that replay. Polling the result
// with a query is not a fix: a query on a session the router has not finished
// registering is never answered *or* refused, so the first `await` blocks forever and
// the retry loop underneath it is unreachable code. That is the hang.
//
// It was read as "an open transaction blocks every other session on this socket",
// which sent every lane, the lease and the relay off to open connections of their own.
// That claim was measured directly and is false: with 64 transactions open on one
// socket, a new session on it signs in in ~54ms and gets a permission-requiring
// statement back in ~5ms, whether it was created before or after those transactions
// began. What sharing a socket does cost is `bus/examples/clone_cost`: +27.5ms per
// transaction for the replayed sign-in, against a ~1ms empty transaction on a
// connection of its own.
//
// So there is nothing to warm. [`begin`] clones a session per transaction and lets the
// replay ride ahead of `Begin` on the same ordered socket, which is what the whole
// codebase now runs on — one connection per process.

/// Opens a transaction for one write.
///
/// `Surreal::begin` consumes its client and hands it back on commit, so a
/// long-lived shared connection cannot serve two concurrent transactions. Request
/// handlers are concurrent by definition, hence a session per write.
///
/// **The only place a transaction is opened**, request path and projectors alike.
/// A projector lane used to hold its own connection and cycle it through
/// `begin`/`commit`; it calls this per event now, on the connection every other lane
/// shares.
///
/// ponytail: `Surreal::clone` mints a session and replays `Attach`/`Signin`/`Use`
/// onto it, so this costs a few round trips per *write* — reads never come here.
/// If write latency ever shows it, the fix is a small pool of pre-authenticated
/// clients handed out per transaction; the reason it is not one already is that a
/// pool needs a guard that returns the client on every path, including the `?`
/// early-returns these handlers are full of, and `Drop` cannot await a rollback.
pub async fn begin(db: &Surreal<Client>) -> MyResult<Transaction<Client>> {
    Ok(db.clone().begin().await?)
}

/// The version this aggregate will have **after** the caller's write.
///
/// Must be called inside the transaction that then writes the row, because the
/// read and the write together are the concurrency control: two writers both see
/// version 3, both write 4 to the same record, and TiKV refuses one of them. Call
/// it outside the transaction and that guarantee is gone.
///
/// Starts at 1 for a row that does not exist yet.
pub async fn next_version(tx: &impl Querier, table: &str, id: &Uuid) -> MyResult<u64> {
    let current: Option<i64> = tx
        .q("SELECT VALUE version FROM ONLY type::record($t, $i)")
        .bind(("t", table.to_string()))
        .bind(("i", *id))
        .await?
        .take(0)?;
    Ok(current.unwrap_or(0).max(0) as u64 + 1)
}

/// Records the aggregate version a projector just applied.
///
/// Called by the projector rather than by `bus::Tx`, because only the projector
/// knows which table the aggregate lives in *here*: view-service holds `payout`
/// but no `payment`, and an `UPDATE` naming a table this database does not have is
/// an error, not a no-op. (A missing *row* in a table that exists is a clean
/// no-op — verified against SurrealDB 3.2.4.)
///
/// `WHERE version < $v` so a redelivered or out-of-order event cannot wind the
/// version backwards. A client waiting on `user:<id>@7` must never see 7 and then
/// 6 again.
///
/// ## Version gaps
///
/// Versions are gapless per aggregate — [`next_version`] assigns them inside the
/// writing transaction — so applying `$v` to a row at `$v - 2` means an event was
/// missed, and nothing else in this codebase would have said so: the `UPDATE` below
/// absorbs it, and the status guards downstream drop the transition without a word.
///
/// So it is **logged at error and then applied anyway**. Not fatal, deliberately,
/// because a gap has a legitimate cause: the streams expire (`STREAMS` max_age is
/// seven days), so a consumer created after an aggregate's early events aged out
/// sees its first event at version 5 with nothing before it. Stopping there would
/// wedge that partition permanently on a projection that is merely incomplete, which
/// `POST /internal/backfill` exists to repair.
///
/// A **missing row** is not a gap — that is the create case, and the expired-history
/// case, both of which are silent by design.
///
/// A `backfill` run re-emits current state at its current version, so it will log
/// these by the tableful. Expected; see [`crate::events::Envelope::backfill`].
pub async fn set_version(
    tx: &impl Querier,
    table: &str,
    id: &Uuid,
    version: u64,
) -> MyResult<()> {
    // A read before the write rather than `RETURN BEFORE` on the UPDATE itself: the
    // update is conditional, so it returns nothing at all in exactly the duplicate
    // case this most wants to distinguish from a gap.
    let stored: Option<i64> = tx
        .q("SELECT VALUE version FROM ONLY type::record($t, $i)")
        .bind(("t", table.to_string()))
        .bind(("i", *id))
        .await?
        .take(0)?;

    if let Some(missing) = version_gap(stored.map(|v| v.max(0) as u64), version) {
        tracing::error!(
            aggregate = %format!("{table}:{id}"),
            stored = stored.unwrap_or(0),
            got = version,
            missing,
            "version gap: applying anyway, projection may be incomplete"
        );
    }

    tx.q("UPDATE type::record($t, $i) SET version = $v WHERE version < $v")
        .bind(("t", table.to_string()))
        .bind(("i", *id))
        .bind(("v", version as i64))
        .await?
        .check()?;
    Ok(())
}

/// How many events are missing between `stored` and `incoming`, if any.
///
/// Pure, so the rule can be read and tested without a database — which matters,
/// because every other branch of it is a *silent* one and silence is hard to assert.
///
/// - `None` stored: the row does not exist. The create case, and the
///   expired-history case, neither of which is a gap.
/// - `incoming <= stored`: a duplicate or a redelivery. Not a gap; the caller's
///   `WHERE version < $v` absorbs it.
/// - `incoming == stored + 1`: the ordinary next event.
/// - anything higher: `Some(n)` events were never applied here.
pub fn version_gap(stored: Option<u64>, incoming: u64) -> Option<u64> {
    let stored = stored?;
    incoming.checked_sub(stored + 1).filter(|missing| *missing > 0)
}

/// Whether this error is TiKV refusing a transaction because someone else wrote
/// first — a retry, not a fault.
///
/// **The one place this is decided.** Every caller that retries a contended write
/// asks here, so there is a single string to fix if it ever changes.
///
/// A string match, reluctantly, and it is worth knowing why rather than assuming
/// it is laziness. The conflict arrives as `surrealdb::Error { code: -32000,
/// details: Internal, message }` — and `Internal` is explicitly the
/// `#[surreal(other)]` catch-all for unrecognised kinds, so matching *it* would
/// also catch every unrelated internal failure and retry things that will never
/// succeed. The structured `WriteConflict` exists inside the message text and
/// nowhere else. Worse, the same conflict surfaces as `kind: Query` over HTTP
/// `/sql` but `details: Internal` over the WebSocket client, so even the kind is
/// not stable across access paths.
///
/// Matches `WriteConflict` and not `"Multiple key errors"`: the sibling
/// `already_exist` / `deadlock` / `commit_ts_expired` fields of the same
/// `KeyError` produce that phrase too, and those must not be retried blindly.
///
/// Version-pinned to surrealdb 3.2.4. `bus/examples/tikv_spike.rs` produces a real
/// conflict on demand, which is what should be run after any SurrealDB upgrade — a
/// reworded message would silently turn every contended booking into a 500.
pub fn is_write_conflict(e: &MyError) -> bool {
    matches!(e, MyError::Database(_)) && e.to_string().contains("WriteConflict")
}

/// Anything a query can be issued against.
///
/// The point is that a generated model method does not care whether it is running
/// standalone or as one statement inside an open transaction — `Surreal<Client>`
/// and `Transaction<Client>` both satisfy this, so the same `User::patch(…)` call
/// works in either position.
pub trait Querier {
    fn q<'a>(&'a self, sql: impl Into<Cow<'a, str>>) -> Query<'a, Client>;
}

impl Querier for Surreal<Client> {
    fn q<'a>(&'a self, sql: impl Into<Cow<'a, str>>) -> Query<'a, Client> {
        self.query(sql)
    }
}

impl Querier for Transaction<Client> {
    fn q<'a>(&'a self, sql: impl Into<Cow<'a, str>>) -> Query<'a, Client> {
        self.query(sql)
    }
}

/// So a repository can be built over a *borrowed* querier. This is what lets a
/// projector do `UserRepository { q: &tx }` per event.
impl<T: Querier + ?Sized> Querier for &T {
    fn q<'a>(&'a self, sql: impl Into<Cow<'a, str>>) -> Query<'a, Client> {
        (**self).q(sql)
    }
}

/// The shared-connection case, and the one every long-lived repository should
/// use.
///
/// `Surreal` is already `Arc<Inner>` internally, but its `Clone` is **not** a
/// refcount bump: it mints a new session id and has the engine replay every
/// `replayable()` command onto it — `Attach`, `Signin`, `Use`. So handing
/// `db.clone()` to five things costs five sessions and five root sign-ins over
/// the socket. `Arc<Surreal<Client>>` clones the pointer instead, and everything
/// shares one session.
///
/// Safe here precisely because nothing re-authenticates per request: the
/// connection signs in once at boot (see [`connect`]) and every query carries its
/// own `bind` parameters rather than session-level `SET`. A codebase that called
/// `use_ns` or `authenticate` per request would need the separate sessions.
impl<T: Querier + ?Sized> Querier for std::sync::Arc<T> {
    fn q<'a>(&'a self, sql: impl Into<Cow<'a, str>>) -> Query<'a, Client> {
        (**self).q(sql)
    }
}

// `Cursor` is gone, along with the `_projection` table it wrote.
//
// It recorded how far a projection had consumed its stream, and had to be bumped
// in the same transaction as the data — a cursor that ran ahead of the rows would
// silently skip events on restart. That was necessary while each replica owned a
// private database and had to track where *its own copy* had reached.
//
// Every replica now shares one database and pulls from one durable consumer per
// partition, so the position lives in NATS: an unacked message is redelivered and
// reapplied, and every apply is idempotent. `/readyz` reads whether every partition
// is drained; a client's read-your-own-writes reads the `version` column above.

#[cfg(test)]
mod tests {
    use super::*;

    /// The four cases, and which of them is worth a line in the log.
    ///
    /// Only the last is: the other three are ordinary and would drown it.
    #[test]
    fn only_a_skipped_version_counts_as_a_gap() {
        // No row: a create, or a projection built after this aggregate's early
        // events aged out of the stream. Both are silent by design.
        assert_eq!(version_gap(None, 1), None);
        assert_eq!(version_gap(None, 9), None);

        // The ordinary case.
        assert_eq!(version_gap(Some(0), 1), None);
        assert_eq!(version_gap(Some(6), 7), None);

        // A duplicate or a redelivery. `WHERE version < $v` absorbs these, and they
        // are expected often enough that reporting them would be noise.
        assert_eq!(version_gap(Some(7), 7), None);
        assert_eq!(version_gap(Some(7), 3), None);

        // Genuinely missed events — the one case nothing else in the system says
        // anything about.
        assert_eq!(version_gap(Some(16), 18), Some(1));
        assert_eq!(version_gap(Some(1), 5), Some(3));
    }
}
