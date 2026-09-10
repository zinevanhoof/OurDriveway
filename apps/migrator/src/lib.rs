//! Every database's schema, applied by one process.
//!
//! **No service ever migrates.** That is not a preference, it is a consequence of leaving
//! sqlx: `sqlx::migrate` takes an advisory lock around the whole run, so every replica
//! could safely do it at boot — which is what `k8s/README.md` still describes.
//! `diesel_migrations` takes **no lock at all** (verified against 2.3.2: `advisory` and
//! `lock` appear nowhere in its source). Each migration is wrapped in a transaction, but
//! the *run* is not exclusive, so two replicas booting together would both read the same
//! pending set and both apply it. Centralising here is what puts that guarantee back.
//!
//! Five databases, one cluster. `media` and `notification` have none.
//!
//! ## What this owns that services no longer do
//!
//! `CREATE DATABASE` moved here too. A service cannot create a database from inside
//! itself, so `shared::db::connect` used to open a second connection to the `yugabyte`
//! maintenance database on the miss path. Doing that once, in the one process that has to
//! reach every database anyway, is unremarkable; doing it in five service boots was the
//! thing that kept dragging maintenance concerns into the request path.

use diesel::sql_types::Text;
use diesel_async::AsyncMigrationHarness;
use diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl};
use diesel_migrations::{EmbeddedMigrations, MigrationHarness, embed_migrations};

/// The databases, in the order they are migrated.
///
/// Deterministic on purpose: a failure part-way through must be reproducible, and there is
/// no distributed transaction across five databases to make it atomic. The order carries
/// no dependency — the databases share nothing — it exists so two runs fail identically.
///
/// A function rather than a `const` slice: `EmbeddedMigrations` is neither `Copy` nor
/// `Clone`, so nothing can be moved out of a `&'static [Database]`. `const` items are
/// inlined at each use site, so naming `USER` here mints a fresh value per call.
pub fn databases() -> [Database; 5] {
    [
        Database {
            name: "user",
            env: "USER_DATABASE_URL",
            migrations: USER,
        },
        Database {
            name: "spot",
            env: "SPOT_DATABASE_URL",
            migrations: SPOT,
        },
        Database {
            name: "booking",
            env: "BOOKING_DATABASE_URL",
            migrations: BOOKING,
        },
        Database {
            name: "payment",
            env: "PAYMENT_DATABASE_URL",
            migrations: PAYMENT,
        },
        Database {
            name: "view",
            env: "VIEW_DATABASE_URL",
            migrations: VIEW,
        },
    ]
}

/// Embedded, not read from disk: the runtime image carries no `migrations/` directory and
/// no diesel CLI. The path is resolved at compile time, relative to this crate.
const USER: EmbeddedMigrations = embed_migrations!("../../migrations/user");
const SPOT: EmbeddedMigrations = embed_migrations!("../../migrations/spot");
const BOOKING: EmbeddedMigrations = embed_migrations!("../../migrations/booking");
const PAYMENT: EmbeddedMigrations = embed_migrations!("../../migrations/payment");
const VIEW: EmbeddedMigrations = embed_migrations!("../../migrations/view");

/// One service's database: what it is called, where its URL comes from, and its own
/// migration history.
///
/// Each keeps a separate `__diesel_schema_migrations`. Nothing is merged — a migration
/// belongs to exactly one database, and `view` re-declaring a table `user` also has is
/// two independent histories, not a conflict.
pub struct Database {
    pub name: &'static str,
    pub env: &'static str,
    pub migrations: EmbeddedMigrations,
}

#[derive(Debug)]
pub enum Error {
    /// The database's URL is not in the environment. Named, never printed — a URL carries
    /// the password.
    MissingUrl {
        database: &'static str,
        env: &'static str,
    },
    Connect {
        database: &'static str,
        source: String,
    },
    Create {
        database: &'static str,
        source: String,
    },
    /// A migration failed. `database` and the harness's message are both surfaced,
    /// because "migration failed" without which one is unactionable.
    Migrate {
        database: &'static str,
        source: String,
    },
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingUrl { database, env } => {
                write!(f, "[{database}] {env} is not set")
            }
            Self::Connect { database, source } => write!(f, "[{database}] connect: {source}"),
            Self::Create { database, source } => write!(f, "[{database}] create: {source}"),
            Self::Migrate { database, source } => write!(f, "[{database}] migrate: {source}"),
        }
    }
}

impl std::error::Error for Error {}

/// Migrate every database, in [`DATABASES`] order, stopping at the first failure.
///
/// **Deliberately not atomic, and not rolled back.** There is no transaction spanning five
/// databases, so a failure at `booking` leaves `user` and `spot` at their new versions and
/// `payment` and `view` untouched. Unwinding the two that succeeded would be a second set
/// of migrations that can themselves fail; the supported recovery is to fix the broken
/// migration and run this again, which is a no-op for everything already applied.
pub async fn run_all() -> Result<(), Error> {
    for db in databases() {
        run_one(db).await?;
    }
    Ok(())
}

/// [`run_one`] for the named database, **at most once in this process**.
///
/// For the `#[ignore]`d live tests, which are the only concurrent caller this crate has.
/// `cargo test` runs test threads in parallel and each one's `db()` helper wants a
/// migrated database, so without this they all race — and on YugabyteDB a losing racer
/// does not wait, it fails:
///
/// ```text
/// could not serialize access due to concurrent update (query layer retries aren't
/// supported for multi-statement queries issued via the simple query protocol)
/// ```
///
/// One thread applies the migration, six panic, and the failure names the migration rather
/// than the race. It only ever bit on the *first* run of a new migration, which is exactly
/// when it is least welcome.
///
/// Production has no use for this — the binary runs once, alone, by construction — and it
/// deliberately does not make concurrent migration *safe*: two processes still race. It
/// serialises the threads of one test binary, which is the whole problem it has.
pub async fn ensure(name: &str) -> Result<(), Error> {
    use std::collections::HashMap;
    use tokio::sync::{Mutex, OnceCell};

    static DONE: OnceCell<Mutex<HashMap<String, ()>>> = OnceCell::const_new();

    let done = DONE
        .get_or_init(|| async { Mutex::new(HashMap::new()) })
        .await;

    // The lock is held across the migration on purpose: the point is that the second
    // caller waits for the first to finish rather than starting its own run.
    let mut done = done.lock().await;
    if done.contains_key(name) {
        return Ok(());
    }

    let db = databases()
        .into_iter()
        .find(|d| d.name == name)
        .unwrap_or_else(|| panic!("`{name}` is not one of the migrator's databases"));

    run_one(db).await?;
    done.insert(name.to_string(), ());
    Ok(())
}

/// Create the database if it is not there, then apply whatever it is missing.
pub async fn run_one(db: Database) -> Result<(), Error> {
    let url = std::env::var(db.env).map_err(|_| Error::MissingUrl {
        database: db.name,
        env: db.env,
    })?;

    tracing::info!(database = db.name, "connecting");
    ensure_database(db.name, &url).await?;

    let conn = AsyncPgConnection::establish(&url)
        .await
        .map_err(|e| Error::Connect {
            database: db.name,
            source: e.to_string(),
        })?;

    // `AsyncMigrationHarness` runs diesel's synchronous migration machinery over an async
    // connection. It is what keeps this crate off sync `diesel::PgConnection`, which would
    // need `diesel/postgres` -> pq-sys -> libpq, unified across the whole workspace.
    //
    // ⚠ It does that with `tokio::task::block_in_place`, which PANICS on a current-thread
    // runtime. Any caller needs the multi-threaded flavour — `main` builds
    // `Runtime::new()` rather than `#[tokio::main(flavor = "current_thread")]`, and every
    // test calling this needs `#[tokio::test(flavor = "multi_thread")]`. The bare
    // `#[tokio::test]` default is current-thread, and the panic points into
    // diesel-async's source rather than at your test, so it is worth knowing up front.
    let mut harness = AsyncMigrationHarness::new(conn);

    let applied = harness
        .run_pending_migrations(db.migrations)
        .map_err(|e| Error::Migrate {
            database: db.name,
            source: e.to_string(),
        })?;

    if applied.is_empty() {
        tracing::info!(database = db.name, "up to date");
    } else {
        for version in &applied {
            tracing::info!(database = db.name, %version, "applied");
        }
        tracing::info!(database = db.name, count = applied.len(), "migrated");
    }

    Ok(())
}

/// `CREATE DATABASE` if `pg_database` does not already have it.
///
/// Checked rather than caught. diesel exposes no SQLSTATE — `DatabaseErrorInformation`
/// has no `code()`, and `DatabaseErrorKind` has no variant for either "no such database"
/// or "already exists" — so `3D000` and `42P04` cannot be matched the way the sqlx version
/// did. Asking the catalogue is not a workaround for that; it is more precise, because
/// absence is confirmed rather than inferred, and a permission failure stays loud instead
/// of being swallowed as "already exists".
async fn ensure_database(name: &'static str, url: &str) -> Result<(), Error> {
    let (admin_url, database) = maintenance_url(url).ok_or_else(|| Error::Create {
        database: name,
        source: "DATABASE_URL names no database".into(),
    })?;

    let mut admin = AsyncPgConnection::establish(&admin_url)
        .await
        .map_err(|e| Error::Connect {
            database: name,
            source: e.to_string(),
        })?;

    if database_exists(&mut admin, &database).await? {
        return Ok(());
    }

    // Spliced, because Postgres cannot bind an identifier. Any `"` is doubled so the
    // quoting cannot be escaped out of; the value comes from this process's own
    // environment, never from a request.
    let quoted = database.replace('"', "\"\"");
    let created = diesel::sql_query(format!("CREATE DATABASE \"{quoted}\""))
        .execute(&mut admin)
        .await;

    match created {
        Ok(_) => {
            tracing::info!(database = name, "created");
            Ok(())
        }
        // Another migrator won the race — possible under a Job retry, where the previous
        // pod may have created it before failing later. Re-ask the catalogue rather than
        // guess from the error: present means someone else did it, absent means the
        // CREATE genuinely failed and the error is the real one.
        Err(e) => {
            if database_exists(&mut admin, &database).await? {
                tracing::info!(database = name, "already created by another run");
                Ok(())
            } else {
                Err(Error::Create {
                    database: name,
                    source: e.to_string(),
                })
            }
        }
    }
}

async fn database_exists(conn: &mut AsyncPgConnection, name: &str) -> Result<bool, Error> {
    #[derive(diesel::QueryableByName)]
    struct Found {
        #[diesel(sql_type = Text)]
        #[allow(dead_code)]
        datname: String,
    }

    let rows: Vec<Found> = diesel::sql_query("SELECT datname FROM pg_database WHERE datname = $1")
        .bind::<Text, _>(name)
        .load(conn)
        .await
        .map_err(|e| Error::Create {
            database: "pg_database",
            source: e.to_string(),
        })?;

    Ok(!rows.is_empty())
}

/// Split a URL into (a URL for the always-present `yugabyte` maintenance database, the
/// database name it originally pointed at).
///
/// String surgery on the path only: everything left of the last `/` is copied verbatim, so
/// a password is never parsed and never re-encoded.
///
/// Both splits go the way they do for a reason the unit test pins down. The query is
/// stripped **first**, so a `/` inside `?options=…` cannot be mistaken for the path
/// separator. Then the path is split from the **right**, because the database is the last
/// segment — splitting from the left lands inside a password containing `/`, which is what
/// the `p/a@ss` case in the test caught.
fn maintenance_url(url: &str) -> Option<(String, String)> {
    let (prefix, rest) = url.split_once("://")?;

    let (path, query) = match rest.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (rest, None),
    };

    let (authority, database) = path.rsplit_once('/')?;
    if database.is_empty() || authority.is_empty() {
        return None;
    }

    let mut admin = format!("{prefix}://{authority}/yugabyte");
    if let Some(q) = query {
        admin.push('?');
        admin.push_str(q);
    }
    Some((admin, database.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_database_is_listed_once_with_its_own_env_var() {
        let dbs = databases();
        let mut names: Vec<_> = dbs.iter().map(|d| d.name).collect();
        let mut envs: Vec<_> = dbs.iter().map(|d| d.env).collect();
        names.sort_unstable();
        envs.sort_unstable();
        let unique_names = {
            let mut n = names.clone();
            n.dedup();
            n
        };
        let unique_envs = {
            let mut e = envs.clone();
            e.dedup();
            e
        };

        assert_eq!(names, unique_names, "a database is listed twice");
        assert_eq!(envs, unique_envs, "two databases read the same env var");
        // media and notification have no database; view is easy to forget because the
        // spec's example list omits it.
        assert_eq!(names, ["booking", "payment", "spot", "user", "view"]);
    }

    #[test]
    fn the_maintenance_url_keeps_the_database_name_and_survives_odd_passwords() {
        let (admin, db) = maintenance_url("postgres://yugabyte@localhost:5433/view").unwrap();
        assert_eq!(admin, "postgres://yugabyte@localhost:5433/yugabyte");
        assert_eq!(db, "view");

        // A `/` in the password must not be mistaken for the path separator.
        let (admin, db) =
            maintenance_url("postgres://yugabyte:p/a@ss@yugabyte:5433/booking").unwrap();
        assert_eq!(admin, "postgres://yugabyte:p/a@ss@yugabyte:5433/yugabyte");
        assert_eq!(db, "booking");

        let (admin, db) = maintenance_url("postgres://u@h:5433/payment?sslmode=disable").unwrap();
        assert_eq!(admin, "postgres://u@h:5433/yugabyte?sslmode=disable");
        assert_eq!(db, "payment");

        assert!(maintenance_url("postgres://yugabyte@localhost:5433/").is_none());
        assert!(maintenance_url("not-a-url").is_none());
    }
}
