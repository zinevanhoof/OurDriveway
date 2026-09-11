use std::time::Duration;

use diesel_async::AsyncPgConnection;
use diesel_async::pooled_connection::AsyncDieselConnectionManager;
use diesel_async::pooled_connection::bb8::{Pool, PooledConnection};
use uuid::Uuid;

use crate::error::myerror::{MyError, MyResult};

/// The pool every service holds. `bb8` over `AsyncPgConnection`, replacing `sqlx::PgPool`.
pub type Db = Pool<AsyncPgConnection>;

/// Opens the service's own pool against its own database.
///
/// One `DATABASE_URL` per service, pointing at that service's database inside the
/// one YugabyteDB cluster — `user`, `booking`, `spot`, `payment`, `view`. The
/// database name is what keeps one service's tables out of another's, exactly as
/// `SURREALDB_DB` did.
///
/// **YSQL listens on 5433, not 5432.** A URL that says 5432 fails to connect with a
/// perfectly ordinary "connection refused" and looks like the container is down.
///
/// Authorization is the service's job: verify the JWT with `AuthedJwt`, then check
/// ownership in Rust. There is no row-level security and no per-request identity,
/// because there is no per-request connection.
///
/// ## What replaced the old socket
///
/// This used to be one WebSocket connection per process, with `Surreal::begin`
/// cloning a session per transaction — and `Surreal::clone` is not a refcount bump,
/// it mints a session id and replays `Attach`/`Signin`/`Use` onto it, which
/// `bus/examples/clone_cost` priced at +27.5ms per transaction. A pool is a pool:
/// `pool.begin()` takes an idle connection and gives it back on commit.
/// ## The database is neither created nor migrated here
///
/// Both moved to `apps/migrator`, the one process that touches schema. A service now
/// assumes its database exists and is up to date, and fails loudly at connect if it is
/// not — `3D000` from a pod that started before the migrator Job is a visible failure,
/// which is the point.
///
/// This reverses what used to be here, and the reason is the move off sqlx rather than a
/// change of mind. Self-creation and boot-time migration in every replica were safe
/// *because* `sqlx::migrate` took an advisory lock around the run. `diesel_migrations`
/// takes no lock at all, so `replicas: N` all migrating at boot would race. Centralising
/// is what puts that guarantee back — see `diesel-migration.md`.
pub async fn connect(url: &str) -> MyResult<Db> {
    let manager = AsyncDieselConnectionManager::<AsyncPgConnection>::new(url);

    Pool::builder()
        // Sized for the shape of the work rather than guessed. A service runs
        // PARTITIONS projector lanes, each holding a connection only for as long as
        // one event's transaction, plus request handlers. Yugabyte's per-connection
        // cost is closer to PostgreSQL's than to a thread pool's, so this is
        // deliberately not large.
        .max_size(20)
        .connection_timeout(Duration::from_secs(10))
        .build(manager)
        .await
        .map_err(|e| MyError::Bus(format!("connect: {e}")))
}

/// One connection out of the pool, with the pool's own error folded into [`MyError`].
///
/// Every read used to be `Repo::find(&self.db, …)` — sqlx's `PgExecutor` accepted the
/// pool itself. diesel's does not, so a caller checks out first. This exists so that is
/// one line rather than a `map_err` at ~90 call sites.
///
/// Hold it for as long as the unit of work and no longer: a connection checked out
/// across an `.await` on something that is not the database is a connection the next
/// request cannot have.
pub async fn conn(pool: &Db) -> MyResult<PooledConnection<'_, AsyncPgConnection>> {
    pool.get().await.map_err(|e| MyError::Pool(e.to_string()))
}

// `table_for`, `resolve` and `table_present` were all here, and all three are gone with
// the version functions that used them.
//
// `table_for` was an allowlist mapping an aggregate name to a physical table, and it
// existed because **Postgres cannot bind an identifier**: `SELECT … FROM $1` is a syntax
// error, so the table name had to be spliced into the statement text and therefore could
// never come from a caller unchecked. Nothing splices a table name any more — the version
// macros take a real diesel table, and `bus::await_version` asks each service through its
// own `version_reader!` — so there is no spliced SQL left in this workspace for an
// allowlist to guard.
//
// The one thing it carried that still matters: `user` is a reserved word, so the table is
// `app_user` while the aggregate stays `user`. That mapping now lives where it is used, as
// the `"user" => shared::schema::<svc>::app_user` arm of a service's reader. The version
// `user:<id>@7`, the events and `aggregate_id("user", …)` are all unchanged.
//
// `table_present` asked the catalogue `to_regclass($1)` before every version read and
// write, because a service may project part of a stream without storing the aggregate at
// all — payment-service mirrors two columns of a user into `host` and has no `app_user`.
// It could not be "try it and swallow 42P01": a failed statement ABORTS the surrounding
// transaction, both callers ran inside one, and Postgres answers a COMMIT on an aborted
// transaction with a silent ROLLBACK. That is how payment-service's host mirror once
// consumed eighteen events and stayed empty.
//
// The macros make the question unaskable rather than answering it at runtime: a caller
// names a table from its own `shared::schema::<svc>` module, so a table this database does
// not have is a name that does not exist. That check caught a real one — payment-service's
// projector called `set_version(conn, "user", …)`, which resolved to `app_user`, which is
// not in that database, so it had silently done nothing on every USERS event.
//
// `resolve` was the error arm for an aggregate name outside the allowlist. Also
// unrepresentable now, for the same reason.

/// The version this aggregate will have **after** the caller's write, having first
/// taken a row lock on it.
///
/// ## A macro over a table, not a function over an aggregate name
///
/// This was `next_version(conn, "booking", &id)`: the name went through an allowlist,
/// the table was spliced into a `format!`, and the row came back through a
/// `QueryableByName` struct because `sql_query` deserializes by column *name*.
///
/// Every caller knows its table at compile time, so all of that was runtime machinery for
/// a compile-time fact. As a macro the query is built by the DSL and monomorphised at each
/// call site: no splicing, no allowlist, no `table_present` — a table this service's
/// schema module does not declare is a compile error — and `.select(version)` loads
/// straight into `i64`, which is why there is no struct here any more.
///
/// A macro rather than a generic function because `version` is a column on every table but
/// not on the `Table` *trait*: a generic `fn` would need a `Versioned` trait, six impls,
/// and a `FindDsl`/`SelectDsl`/`LoadQuery` bound stack longer than the query. Expanding at
/// the call site needs none of it.
///
/// ```ignore
/// let version = shared::next_version!(conn, shared::schema::booking::booking, &booking_id)?;
/// ```
///
/// ## The `FOR UPDATE` is the concurrency control, and it is here so it cannot be forgotten
///
/// Under TiKV this was a plain read: two writers both saw version 3, both wrote 4 to
/// the same record, and the store refused one of them. **Read Committed does not
/// refuse it.** The second writer blocks on the row lock, re-reads, and applies — so
/// both would write version 4, both would publish an event at version 4, and the
/// projector's `WHERE version < $v` would apply one and silently drop the other. A
/// downstream projection quietly missing an edit, with no error anywhere.
///
/// So the lock is taken here rather than at each call site: every write path that
/// bumps a version goes through this macro, and a rule applied in one place is not
/// a rule anyone has to remember.
///
/// Must still be used **inside** the transaction that then writes the row — the
/// lock is released at commit, and a lock taken in a different transaction protects
/// nothing.
///
/// Returns 1 for a row that does not exist yet. There is nothing to lock in that
/// case and nothing to race either: a create mints its own uuid.
///
/// `.max(0)` because the column is signed and this is what a negative one becomes: a row
/// that looks fresh, so the next event applies. It cannot come from this application, and
/// the alternative — trusting it — is a `next` below every version already stored.
///
/// Two write paths do **not** come through here and need their own lock, because in
/// both the racing writers touch different rows and nothing collides:
///   - reserve, which locks the spot mirror row (`booking_service`);
///   - `request_payout`, which takes `pg_advisory_xact_lock` on the host id.
#[macro_export]
macro_rules! next_version {
    // `$($table:ident)::+` and not `$table:path`: a `path` fragment is an opaque AST node
    // that cannot have `::table` appended to it, so the module has to arrive as the
    // sequence of idents it is.
    ($conn:expr, $($table:ident)::+, $id:expr) => {{
        // Imported inside the expansion, and `as _` so the call site's own imports cannot
        // clash — `diesel::prelude::*` would drag in the SYNC `RunQueryDsl` and make
        // `.first()` ambiguous against the async one.
        use ::diesel::OptionalExtension as _;
        use ::diesel::QueryDsl as _;
        use ::diesel_async::RunQueryDsl as _;

        $($table)::+::table
            .find($id)
            .select($($table)::+::version)
            .for_update()
            .first::<i64>($conn)
            .await
            .optional()
            .map(|stored| stored.unwrap_or(0).max(0) + 1)
    }};
}

/// Records the aggregate version a projector just applied.
///
/// The macro half of [`next_version!`] — same reasoning, same expansion rules. The
/// aggregate NAME is still a parameter, because it is a protocol identifier rather than a
/// table: it is what the log line names and what a client's version (`user:<id>@7`)
/// spells, and for `user` it differs from the table (`app_user`) on purpose.
///
/// `WHERE version < $v` so a redelivered or out-of-order event cannot wind the
/// version backwards. A client waiting on `user:<id>@7` must never see 7 and then 6.
///
/// A missing *row* is a clean no-op. A missing *table* is no longer possible: the caller
/// names a table its own schema module declares, which is the check `table_present` used
/// to do at runtime — see the note in [`next_version!`].
///
/// ## Version gaps
///
/// Versions are gapless per aggregate — [`next_version!`] assigns them inside the
/// writing transaction — so applying `$v` to a row at `$v - 2` means an event was
/// missed, and nothing else in this codebase would say so: the `UPDATE` absorbs it
/// and the status guards downstream drop the transition without a word.
///
/// So it is **logged at error and then applied anyway**. Not fatal, deliberately: the
/// streams expire (seven days), so a consumer created after an aggregate's early
/// events aged out legitimately sees its first event at version 5. Stopping there
/// would wedge that partition on a projection that is merely incomplete, which
/// `POST /internal/backfill` exists to repair.
///
/// A backfill run re-emits current state at its current version, so it will log these
/// by the tableful. Expected.
///
/// ```ignore
/// shared::set_version!(conn, "spot", shared::schema::view::spot, &spot_id, version)?;
/// ```
#[macro_export]
macro_rules! set_version {
    // See the note on the matcher in [`next_version!`].
    ($conn:expr, $aggregate:expr, $($table:ident)::+, $id:expr, $version:expr) => {{
        use ::diesel::ExpressionMethods as _;
        use ::diesel::OptionalExtension as _;
        use ::diesel::QueryDsl as _;
        use ::diesel_async::RunQueryDsl as _;

        async {
            // A read before the write rather than `RETURNING`: the update is conditional,
            // so it returns nothing at all in exactly the duplicate case this most wants
            // to distinguish from a gap.
            let stored: Option<i64> = $($table)::+::table
                .find($id)
                .select($($table)::+::version)
                .first::<i64>(&mut *$conn)
                .await
                .optional()?;

            if let Some(missing) = $crate::db::version_gap(stored, $version) {
                ::tracing::error!(
                    aggregate = %format!("{}:{}", $aggregate, $id),
                    stored = stored.unwrap_or(0),
                    got = $version,
                    missing,
                    "version gap: applying anyway, projection may be incomplete"
                );
            }

            ::diesel::update($($table)::+::table.find($id))
                .filter($($table)::+::version.lt($version))
                .set($($table)::+::version.eq($version))
                .execute(&mut *$conn)
                .await?;

            Ok::<(), $crate::error::myerror::MyError>(())
        }
        .await
    }};
}


/// How many events are missing between `stored` and `incoming`, if any.
///
/// Pure, so the rule can be read and tested without a database — which matters,
/// because every other branch of it is a *silent* one and silence is hard to assert.
///
/// - `None` stored: the row does not exist. The create case, and the expired-history
///   case, neither of which is a gap.
/// - `incoming <= stored`: a duplicate or a redelivery. Not a gap; the caller's
///   `WHERE version < $v` absorbs it.
/// - `incoming == stored + 1`: the ordinary next event.
/// - anything higher: `Some(n)` events were never applied here.
///
/// Signed, like the column it compares against — and the `> 0` filter is what makes the
/// duplicate and redelivery cases fall out either way, so the sign never decided anything
/// here. `checked_sub` stays because `stored + 1` on `i64::MAX` is the one overflow left,
/// unreachable but free to rule out.
pub fn version_gap(stored: Option<i64>, incoming: i64) -> Option<i64> {
    let stored = stored?;
    incoming
        .checked_sub(stored.saturating_add(1))
        .filter(|missing| *missing > 0)
}

/// A per-host lock key for `pg_advisory_xact_lock`, from the low 64 bits of a uuid.
///
/// Used by `request_payout`, which has nothing to lock: a balance is derived
/// (`earnings − Σ payouts`), never stored, so two double-clicked withdrawals insert
/// two *different* payout rows and nothing collides. Locking the existing payout rows
/// does not help either — the row that changes the answer is one that does not exist
/// yet, and a first-time withdrawer has none. That phantom is what Read Committed
/// permits, and it is why a `host` table used to exist purely to be bumped.
///
/// The lock is taken on the host id itself instead, so the table is gone.
///
/// Xact-scoped (`_xact_`, never the session variant): released by COMMIT or ROLLBACK,
/// so there is no unlock to forget on the `?` early-returns these handlers are full
/// of — and no `Drop` that would have to await one.
///
/// A hash collision between two hosts is possible and harmless: they would briefly
/// serialise against each other, which costs a wait and never correctness. Across 64
/// bits of a v4/v7 uuid it is not worth engineering around.
pub fn advisory_key(id: &Uuid) -> i64 {
    id.as_u64_pair().1 as i64
}

/// Whether this error is the database refusing a transaction that raced another —
/// a retry, not a fault.
///
/// **A backstop, not the mechanism.** Under Read Committed the ordinary contended
/// write does not error at all: the second writer blocks, re-reads and applies. What
/// keeps that correct is locking (see [`next_version`] and [`advisory_key`]), not
/// this. What is left for this to catch is a genuine serialization failure and a
/// deadlock, both of which are safe to retry once.
///
/// This replaced a string match on `"WriteConflict"` — the only surface a TiKV
/// conflict had, because it arrived as an untyped `Internal` error whose kind was not
/// even stable across access paths, with `bus/examples/tikv_spike.rs` existing purely
/// to re-verify that string after an upgrade.
///
/// ## Why this is one enum arm and not two SQLSTATEs
///
/// It used to match `40001` *or* `40P01`. diesel exposes no SQLSTATE at all —
/// `DatabaseErrorInformation` has no `code()` — so only what `DatabaseErrorKind` names
/// is reachable, and that covers `40001` (`SerializationFailure`, mapped by diesel-async)
/// but not `40P01` (deadlock), which lands in `Unknown`.
///
/// Dropped rather than recovered by matching the message, because a deadlock is not
/// reachable here. The lock ordering is one-way everywhere: `reserve` takes the spot
/// mirror row `FOR UPDATE` and then `next_version` on a brand-new `Uuid::now_v7()` that
/// locks nothing, `transition` takes a booking row alone, and `request_payout` takes the
/// advisory lock and then rows. Nothing acquires that pair in the opposite order, so
/// there is no cycle to detect.
///
/// Worth knowing if that ever changes: nothing calls this today. There is no
/// transaction-retry loop in the workspace — the event paths retry by NATS redelivery —
/// so it is a backstop waiting for a caller, and the first one to appear should re-check
/// the paragraph above rather than trust it.
pub fn is_write_conflict(e: &MyError) -> bool {
    matches!(
        e,
        MyError::Database(diesel::result::Error::DatabaseError(
            diesel::result::DatabaseErrorKind::SerializationFailure,
            _
        ))
    )
}

// `migrate` is gone. Every service used to apply its own migrations at boot from
// `sqlx::migrate!`, which was safe only because sqlx locks around the run.
// `diesel_migrations` does not lock, so that became a race at `replicas: N`. Schema is
// now `apps/migrator`'s job alone — one Compose one-shot in dev, one Helm hook Job in
// production — and nothing in a service's startup path touches it.

// `Cursor` is still gone, along with the `_projection` table it wrote: every replica
// shares one database and pulls from one durable consumer per partition, so the
// position lives in NATS. An unacked message is redelivered and reapplied, and every
// apply is idempotent.

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

        // Genuinely missed events — the one case nothing else says anything about.
        assert_eq!(version_gap(Some(16), 18), Some(1));
        assert_eq!(version_gap(Some(1), 5), Some(3));

        // A stored version below zero is reachable in the type now that this is signed,
        // and it reaches here unclamped: `set_version` reports the gap and applies
        // anyway, so the only consequence is a larger number in a log line about a row
        // nothing in this application could have written. `next_version` is where a
        // negative is clamped, because there it would hand back a version *below* one
        // already stored.
        assert_eq!(version_gap(Some(-3), 5), Some(7));
    }

    /// The allowlist is what stands between an aggregate name and spliced SQL, so
    /// it must answer for exactly the names the services use and nothing else.
    // `every_aggregate_resolves_and_nothing_else_does` was here, over `table_for`. It
    // asserted that six aggregate names mapped to a table and that everything else —
    // `""`, `"app_user"`, `"spot; DROP TABLE spot--"` — mapped to `None`, because the
    // answer was about to be spliced into SQL.
    //
    // There is nothing left to assert. No table name is spliced anywhere: the version
    // macros take a real diesel table, and an aggregate a service does not store is an
    // absent match arm in its `version_reader!` rather than a string that fails a lookup.
    // The injection case in particular is not a test any more, it is a type error.
    //
    // What the test really guarded — that the set of aggregates and the set of tables
    // agree — is now checked by the compiler at every one of those call sites.

    /// Two different hosts must not share a lock key by construction.
    #[test]
    fn advisory_keys_differ_per_host() {
        let (a, b) = (Uuid::now_v7(), Uuid::now_v7());
        assert_ne!(advisory_key(&a), advisory_key(&b));
        assert_eq!(
            advisory_key(&a),
            advisory_key(&a),
            "must be stable per host"
        );
    }
}
