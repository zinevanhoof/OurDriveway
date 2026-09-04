use std::time::Duration;

use sqlx::{
    Connection, PgConnection, PgPool,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use uuid::Uuid;

use crate::error::myerror::{MyError, MyResult};

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
/// ## The database creates itself
///
/// A service's first boot against a fresh cluster finds no database to connect to, so
/// this creates it and retries once. That cannot be a migration: `sqlx::migrate!` runs
/// its files on a connection to the database being migrated, so a missing one fails at
/// connect and the files are never read. (The transaction is *not* the obstacle — sqlx
/// honours a leading `-- no-transaction`, which is what `CREATE DATABASE` would need.)
///
/// It replaced a Helm `post-install` hook that looped over the service list and created
/// all five centrally. Three things improve by moving it here:
///
/// * There is one list of databases — the `DATABASE_URL`s themselves — instead of a
///   second one in a chart that could disagree with them.
/// * `docker compose` needs no equivalent. It never had one, so `down -v` deleted the
///   databases and every service then failed at boot with `3D000`.
/// * A service no longer waits on a hook that runs *after* it starts, which was a
///   CrashLoopBackOff on every fresh install.
///
/// It assumes the role in `DATABASE_URL` may create databases. Both environments
/// connect as `yugabyte` — the dev `.env`s and `k8s/chart/templates/services.yaml` —
/// so it may. Give the services a non-superuser role and this is the line that stops
/// working, loudly, on a fresh cluster only.
pub async fn connect(url: &str) -> MyResult<PgPool> {
    match pool(url).await {
        Err(e) if is_missing_database(&e) => {
            create_database(url).await?;
            pool(url).await
        }
        other => other,
    }
}

async fn pool(url: &str) -> MyResult<PgPool> {
    Ok(PgPoolOptions::new()
        // Sized for the shape of the work rather than guessed. A service runs
        // PARTITIONS projector lanes, each holding a connection only for as long as
        // one event's transaction, plus request handlers. Yugabyte's per-connection
        // cost is closer to PostgreSQL's than to a thread pool's, so this is
        // deliberately not large.
        .max_connections(20)
        .acquire_timeout(Duration::from_secs(10))
        .connect(url)
        .await?)
}

/// SQLSTATE 3D000: the server is up and reachable, and has no such database.
///
/// Deliberately narrow. A refused connection, a bad password or the wrong port must
/// keep failing as themselves — creating a database is only ever the answer to *this*.
fn is_missing_database(e: &MyError) -> bool {
    matches!(
        e,
        MyError::Database(sqlx::Error::Database(d)) if d.code().as_deref() == Some("3D000")
    )
}

/// Creates the database named in `url`, from a connection to the maintenance one.
///
/// The name is taken back off the parsed options rather than sliced out of the URL, so
/// a password containing a `/` cannot confuse it.
async fn create_database(url: &str) -> MyResult<()> {
    let opts: PgConnectOptions = url
        .parse()
        .map_err(|e| MyError::Bus(format!("DATABASE_URL is not a postgres URL: {e}")))?;
    let name = opts
        .get_database()
        .ok_or_else(|| MyError::Bus("DATABASE_URL names no database".to_string()))?
        .to_owned();

    // `yugabyte` is the cluster's own always-present database, the same one the Helm
    // hook used to connect to. `postgres` would do; this matches the role name the
    // URLs already carry.
    let mut admin = PgConnection::connect_with(&opts.clone().database("yugabyte")).await?;

    // Spliced, because Postgres cannot bind an identifier — the same constraint
    // `table_for` exists for. Quoted because `user` is a reserved word, and any `"` in
    // the name is doubled so the quoting cannot be escaped out of. The value comes from
    // this service's own environment, never from a request.
    let quoted = name.replace('"', "\"\"");
    match sqlx::query(&format!("CREATE DATABASE \"{quoted}\""))
        .execute(&mut admin)
        .await
    {
        Ok(_) => {
            tracing::info!(database = %name, "created");
            Ok(())
        }
        // 42P04: another replica booting at the same time won the race. That is the
        // outcome we wanted, so it is a success rather than something to retry.
        Err(sqlx::Error::Database(d)) if d.code().as_deref() == Some("42P04") => {
            tracing::info!(database = %name, "already created by another replica");
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
}

/// The physical table an aggregate name maps to.
///
/// An allowlist and not string interpolation, because **Postgres cannot bind an
/// identifier**: `SELECT … FROM $1` is a syntax error, so the table name has to be
/// spliced into the statement text and therefore must never come from a caller
/// unchecked. Every name here is a literal in this file.
///
/// It also carries the one rename in the schema. `user` is a reserved word, and the
/// failure is silent rather than loud — an unquoted `FROM user` reads the
/// current-user keyword and returns a row instead of erroring — so the table is
/// `app_user` while the aggregate stays `user`: the await token `user:<id>@7`, the
/// events and `aggregate_id("user", …)` are all unchanged.
///
/// `None` means the caller passed a name no service owns, which is a typo and is
/// reported. It does **not** mean "this database has no such table" — that is a
/// legitimate case and is handled by [`table_present`].
pub fn table_for(aggregate: &str) -> Option<&'static str> {
    Some(match aggregate {
        "user" => "app_user",
        "spot" => "spot",
        "booking" => "booking",
        "payment" => "payment",
        "payout" => "payout",
        "refresh_token" => "refresh_token",
        _ => return None,
    })
}

fn resolve(aggregate: &str) -> MyResult<&'static str> {
    table_for(aggregate).ok_or_else(|| {
        MyError::Bus(format!(
            "unknown aggregate {aggregate:?}; see db::table_for"
        ))
    })
}

/// Whether this error is "that table does not exist in this database".
///
/// Not a fault: each service's schema holds only the aggregates it projects, and a
/// version can be asked about anywhere. SQLSTATE 42P01.
///
/// `payment` in view-service used to be the example of this. It is a real table there
/// now — the wallet reads it — so that particular lookup resolves rather than escaping
/// through here.
/// Whether this database has that table, asked of the catalogue.
///
/// # Why not "try it and swallow 42P01"
///
/// Because a failed statement **aborts the surrounding transaction**, and both callers
/// run inside one. Swallowing the error returns `Ok` to a caller that then commits —
/// and Postgres answers a COMMIT on an aborted transaction with a silent ROLLBACK. The
/// writes made before it disappear, the projector acks its message, and nothing
/// reports a problem. `payment-service`'s host mirror consumed eighteen events that way
/// and stayed empty; that is what this function exists to prevent.
///
/// `to_regclass` answers NULL for a table that is not there rather than raising, which
/// is what makes it safe to ask mid-transaction.
async fn table_present(conn: &mut PgConnection, table: &str) -> MyResult<bool> {
    let found: Option<String> = sqlx::query_scalar("SELECT to_regclass($1)::text")
        .bind(table)
        .fetch_one(conn)
        .await?;
    Ok(found.is_some())
}

/// The version this aggregate will have **after** the caller's write, having first
/// taken a row lock on it.
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
/// bumps a version goes through this function, and a rule applied in one place is not
/// a rule anyone has to remember.
///
/// Must still be called **inside** the transaction that then writes the row — the
/// lock is released at commit, and a lock taken in a different transaction protects
/// nothing.
///
/// Returns 1 for a row that does not exist yet. There is nothing to lock in that
/// case and nothing to race either: a create mints its own uuid.
///
/// Two write paths do **not** come through here and need their own lock, because in
/// both the racing writers touch different rows and nothing collides:
///   - reserve, which locks the spot mirror row (`booking_service`);
///   - `request_payout`, which takes `pg_advisory_xact_lock` on the owner id.
pub async fn next_version(conn: &mut PgConnection, aggregate: &str, id: &Uuid) -> MyResult<u64> {
    let table = resolve(aggregate)?;

    // Same catalogue check as `set_version`, and the same reason — this also runs
    // inside the caller's write transaction. In practice only the service that owns an
    // aggregate calls this, so the branch is not expected to be taken; it is here so
    // that the failure mode, if it ever is, is "version 1" rather than a transaction
    // that silently commits nothing.
    if !table_present(&mut *conn, table).await? {
        return Ok(1);
    }

    let sql = format!("SELECT version FROM {table} WHERE id = $1 FOR UPDATE");
    let current: Option<i64> = sqlx::query_scalar(&sql).bind(id).fetch_optional(conn).await?;

    Ok(current.unwrap_or(0).max(0) as u64 + 1)
}

/// Records the aggregate version a projector just applied.
///
/// Called by the projector rather than by `bus::Tx`, because only the projector knows
/// which table the aggregate lives in *here*: view-service holds `payout` but no
/// `payment`, and payment-service projects users into a `host` mirror while having no
/// `app_user` at all. Such a table is checked for first — see [`table_present`], which
/// is where the interesting failure is written up — and a missing one makes this a
/// no-op. A missing *row* in a table that does exist is a clean no-op too.
///
/// `WHERE version < $v` so a redelivered or out-of-order event cannot wind the
/// version backwards. A client waiting on `user:<id>@7` must never see 7 and then 6.
///
/// ## Version gaps
///
/// Versions are gapless per aggregate — [`next_version`] assigns them inside the
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
pub async fn set_version(
    conn: &mut PgConnection,
    aggregate: &str,
    id: &Uuid,
    version: u64,
) -> MyResult<()> {
    let table = resolve(aggregate)?;

    // Is this table even in this database? A service may project part of a stream
    // without storing the aggregate itself — payment-service mirrors two columns of a
    // user into `host` and has no `app_user` at all. Asked first, and asked of the
    // catalogue: see [`table_present`] for what happens otherwise.
    if !table_present(&mut *conn, table).await? {
        return Ok(());
    }

    // A read before the write rather than `RETURNING`: the update is conditional, so
    // it returns nothing at all in exactly the duplicate case this most wants to
    // distinguish from a gap.
    let read = format!("SELECT version FROM {table} WHERE id = $1");
    let stored: Option<i64> = sqlx::query_scalar(&read)
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?;

    if let Some(missing) = version_gap(stored.map(|v| v.max(0) as u64), version) {
        tracing::error!(
            aggregate = %format!("{aggregate}:{id}"),
            stored = stored.unwrap_or(0),
            got = version,
            missing,
            "version gap: applying anyway, projection may be incomplete"
        );
    }

    // No missing-table arm here either, and for the same reason: past the check above
    // the table is there, and a genuine failure must reach the caller rather than be
    // absorbed into a transaction that then commits nothing.
    let write = format!("UPDATE {table} SET version = $2 WHERE id = $1 AND version < $2");
    sqlx::query(&write)
        .bind(id)
        .bind(version as i64)
        .execute(conn)
        .await?;
    Ok(())
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
pub fn version_gap(stored: Option<u64>, incoming: u64) -> Option<u64> {
    let stored = stored?;
    incoming
        .checked_sub(stored + 1)
        .filter(|missing| *missing > 0)
}

/// A per-owner lock key for `pg_advisory_xact_lock`, from the low 64 bits of a uuid.
///
/// Used by `request_payout`, which has nothing to lock: a balance is derived
/// (`earnings − Σ payouts`), never stored, so two double-clicked withdrawals insert
/// two *different* payout rows and nothing collides. Locking the existing payout rows
/// does not help either — the row that changes the answer is one that does not exist
/// yet, and a first-time withdrawer has none. That phantom is what Read Committed
/// permits, and it is why a `host` table used to exist purely to be bumped.
///
/// The lock is taken on the owner id itself instead, so the table is gone.
///
/// Xact-scoped (`_xact_`, never the session variant): released by COMMIT or ROLLBACK,
/// so there is no unlock to forget on the `?` early-returns these handlers are full
/// of — and no `Drop` that would have to await one.
///
/// A hash collision between two owners is possible and harmless: they would briefly
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
/// to re-verify that string after an upgrade. SQLSTATE is typed and standard.
pub fn is_write_conflict(e: &MyError) -> bool {
    matches!(
        e,
        MyError::Database(sqlx::Error::Database(d))
            if matches!(d.code().as_deref(), Some("40001") | Some("40P01"))
    )
}

/// Runs a service's migrations against its own database.
///
/// Called by every replica at boot, which is safe: `sqlx` takes an advisory lock
/// around the run, migrations are versioned and applied once, and each service owns
/// its own database so there is no contention between services at all — only between
/// replicas of one, which is exactly what that lock covers.
///
/// This replaced applying schemas from *outside* the application: a ConfigMap of
/// `.surql` files, a `--set-file` loop in `deploy.sh` because Helm cannot read
/// outside its chart directory, and a post-install Job that POSTed them to `/sql`.
/// `sqlx::migrate!` embeds the SQL in the binary, so there is nothing to mount and
/// nothing to keep in step with the image.
pub async fn migrate(pool: &PgPool, migrator: &sqlx::migrate::Migrator) -> MyResult<()> {
    migrator
        .run(pool)
        .await
        .map_err(|e| MyError::Bus(format!("migrate: {e}")))
}

// `Querier` is gone, and so is the alias that briefly stood in for it. It existed so a
// repository method could run against either the shared connection or an open
// transaction, because `Surreal<Client>` and `Transaction<Client>` shared no trait.
// sqlx already has one: `sqlx::PgExecutor` is implemented for `&PgPool` and for
// `&mut PgConnection`, so a method taking `impl PgExecutor<'_>` serves both positions
// with nothing of ours in between. A transaction reaches it as `&mut *tx` — `Transaction`
// derefs to `PgConnection` — which is what every projector and service call site passes.
//
// The one wrinkle is that `PgExecutor` is consumed per statement, so a method issuing
// two of them takes `&mut PgConnection` and reborrows — which is why `next_version`
// and `set_version` above do, and why they are honest about only ever running inside
// a transaction.
//
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
    }

    /// The allowlist is what stands between an aggregate name and spliced SQL, so
    /// it must answer for exactly the names the services use and nothing else.
    #[test]
    fn every_aggregate_resolves_and_nothing_else_does() {
        for aggregate in [
            "user",
            "spot",
            "booking",
            "payment",
            "payout",
            "refresh_token",
        ] {
            assert!(table_for(aggregate).is_some(), "{aggregate}");
        }

        // The rename, and the reason it exists: `user` is reserved, and an unquoted
        // `FROM user` returns the current-user keyword rather than erroring.
        assert_eq!(table_for("user"), Some("app_user"));

        // Anything else is a typo, and must be reported rather than spliced.
        assert_eq!(
            table_for("app_user"),
            None,
            "the aggregate is `user`, not the table name"
        );
        assert_eq!(
            table_for("host"),
            None,
            "the host table is gone; see advisory_key"
        );
        assert_eq!(table_for(""), None);
        assert_eq!(table_for("spot; DROP TABLE spot--"), None);
    }

    /// Two different owners must not share a lock key by construction.
    #[test]
    fn advisory_keys_differ_per_owner() {
        let (a, b) = (Uuid::now_v7(), Uuid::now_v7());
        assert_ne!(advisory_key(&a), advisory_key(&b));
        assert_eq!(
            advisory_key(&a),
            advisory_key(&a),
            "must be stable per owner"
        );
    }
}
