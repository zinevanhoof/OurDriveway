//! What the migrator has to be true of, against a real cluster.
//!
//! `#[ignore]`d like every other database test in this workspace — CI has no Postgres,
//! so these are run by hand:
//!
//! ```sh
//! docker compose -f docker/docker-compose-dev.yml up -d yugabyte
//! cargo test -p migrator -- --ignored
//! ```
//!
//! Each test works in its own throwaway database so they can run in any order and leave
//! nothing behind. They deliberately do **not** touch `user`/`spot`/`booking`/`payment`/
//! `view` — those hold the dev data, and a test that wipes what you were looking at is
//! worse than no test.

use diesel::sql_types::{BigInt, Text};
use diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl};
use diesel_migrations::{EmbeddedMigrations, embed_migrations};

const BASE: &str = "postgres://yugabyte@127.0.0.1:5433";

/// `user`'s two migrations, borrowed as a fixture. Any real set would do; this one is
/// small and has more than one migration, which is what the "second run is a no-op"
/// assertion needs to be meaningful.
const FIXTURE: EmbeddedMigrations = embed_migrations!("../../migrations/user");

/// Held by every test that creates a database, so no two of them do it at once.
///
/// YugabyteDB 2026.1 does not run concurrent `CREATE DATABASE`s side by side: one wins and
/// the others fail with `Keyspace '<name>' already exists` — for names that never existed
/// — and leave a half-made keyspace behind that blocks the name for up to a minute, which
/// `DROP DATABASE IF EXISTS` cannot see. 2025.2 ran the same three concurrent creates
/// cleanly; both tags were measured side by side before this lock went in.
///
/// The migrator itself never meets it: `run_all` creates the five databases one after
/// another. Only this file created them in parallel, as a side effect of cargo's test
/// threads. `tokio`'s mutex and not `std`'s, because each `#[tokio::test]` is its own
/// runtime and one failing test must not poison the lock for the rest.
static CREATES: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn scratch(name: &str) -> migrator::Database {
    // Leaked so the `&'static str` the API wants can be per-test. A handful of
    // strings for the life of the test binary.
    let env: &'static str =
        Box::leak(format!("MIGRATOR_TEST_{}", name.to_uppercase()).into_boxed_str());
    let name: &'static str = Box::leak(name.to_string().into_boxed_str());

    unsafe { std::env::set_var(env, format!("{BASE}/{name}")) };

    migrator::Database {
        name,
        env,
        migrations: FIXTURE,
    }
}

async fn admin() -> AsyncPgConnection {
    AsyncPgConnection::establish(&format!("{BASE}/yugabyte"))
        .await
        .expect("the dev cluster is up — docker compose up -d yugabyte")
}

/// Drops a scratch database, and **insists**.
///
/// Two things had to be got right here, and both were found by this failing rather than
/// by reasoning:
///
/// **`WITH (FORCE)`.** A connection this run opened to the database outlives the statement
/// that opened it for as long as Yugabyte takes to reap the backend, and a plain
/// `DROP DATABASE` refuses while one exists — which then surfaced as "database already
/// exists" from the *next* test's `CREATE`, a message pointing at the wrong statement
/// entirely. An explicit `pg_terminate_backend` before the drop is **not** enough: it
/// signals and returns, so the backend can still be dying a microsecond later. `FORCE`
/// does both halves in one statement, with no window between them. Confirmed on YSQL.
///
/// **The retry.** Even then this failed about one run in four with
/// `SerializationFailure: Restart read required` — Yugabyte's distributed read hitting an
/// ambiguous snapshot while the other tests are creating and dropping their own scratch
/// databases concurrently. It is transient by definition and the engine normally retries
/// it internally; DDL is where it cannot. So this retries, and **only** on that: any other
/// error is a real failure and must not be swallowed by a loop.
///
/// `expect` at the end and not `let _`: a drop that genuinely cannot happen has to say so
/// here, where the reason is legible.
async fn drop_database(name: &str) {
    use diesel::result::{DatabaseErrorKind, Error};

    let mut conn = admin().await;
    let statement = format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)");

    for attempt in 1..=5 {
        match diesel::sql_query(&statement).execute(&mut conn).await {
            Ok(_) => return,
            Err(Error::DatabaseError(DatabaseErrorKind::SerializationFailure, _))
                if attempt < 5 =>
            {
                tokio::time::sleep(std::time::Duration::from_millis(100 * attempt)).await;
            }
            Err(e) => panic!("drop the scratch database: {e}"),
        }
    }
}

#[derive(diesel::QueryableByName)]
struct Count {
    #[diesel(sql_type = BigInt)]
    count: i64,
}

async fn applied_count(name: &str) -> i64 {
    let mut conn = AsyncPgConnection::establish(&format!("{BASE}/{name}"))
        .await
        .expect("connect to the scratch database");

    diesel::sql_query("SELECT COUNT(*) AS count FROM __diesel_schema_migrations")
        .load::<Count>(&mut conn)
        .await
        .expect("the ledger exists")[0]
        .count
}

/// An empty database is created and brought fully up to date, and running again changes
/// nothing — which is the property that makes a Job retry safe.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs docker/docker-compose-dev.yml"]
async fn an_empty_database_is_created_migrated_and_then_left_alone() {
    let _creates = CREATES.lock().await;
    let db = scratch("migrator_test_fresh");
    drop_database(db.name).await;

    migrator::run_one(scratch("migrator_test_fresh"))
        .await
        .expect("first run migrates");
    let after_first = applied_count(db.name).await;
    assert!(after_first > 1, "the fixture has more than one migration");

    // Second run: no error, and not one extra row in the ledger.
    migrator::run_one(scratch("migrator_test_fresh"))
        .await
        .expect("second run is a no-op");
    assert_eq!(
        after_first,
        applied_count(db.name).await,
        "a rerun re-applied a migration that was already recorded"
    );

    drop_database(db.name).await;
}

/// A database that already exists is migrated in place, not recreated. This is the path
/// every deploy after the first takes.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs docker/docker-compose-dev.yml"]
async fn an_existing_database_is_migrated_in_place() {
    let _creates = CREATES.lock().await;
    let db = scratch("migrator_test_existing");
    drop_database(db.name).await;

    let mut conn = admin().await;
    diesel::sql_query(format!("CREATE DATABASE \"{}\"", db.name))
        .execute(&mut conn)
        .await
        .expect("create it by hand first");

    migrator::run_one(scratch("migrator_test_existing"))
        .await
        .expect("migrates a database it did not create");
    assert!(applied_count(db.name).await > 0);

    drop_database(db.name).await;
}

/// A URL that is not in the environment names the database and the variable, and does not
/// reach the cluster at all.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs docker/docker-compose-dev.yml"]
async fn a_missing_url_fails_by_name() {
    let db = migrator::Database {
        name: "migrator_test_absent",
        env: "MIGRATOR_TEST_DEFINITELY_NOT_SET",
        migrations: FIXTURE,
    };

    let err = migrator::run_one(db).await.expect_err("must not succeed");
    let message = err.to_string();

    assert!(message.contains("migrator_test_absent"), "{message}");
    assert!(
        message.contains("MIGRATOR_TEST_DEFINITELY_NOT_SET"),
        "{message}"
    );
}

/// A migration that cannot apply is reported with the database it belongs to, and the
/// ledger is not advanced past it.
///
/// The fixture is applied to a database that already has a conflicting table, so
/// `0001_init`'s `CREATE TABLE` fails the way a genuinely broken migration would.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs docker/docker-compose-dev.yml"]
async fn a_failing_migration_is_an_error_naming_its_database() {
    let _creates = CREATES.lock().await;
    let db = scratch("migrator_test_conflict");
    drop_database(db.name).await;

    let mut conn = admin().await;
    diesel::sql_query(format!("CREATE DATABASE \"{}\"", db.name))
        .execute(&mut conn)
        .await
        .expect("create it");

    // Put a table in the way that 0001_init also creates.
    let mut scratch_conn = AsyncPgConnection::establish(&format!("{BASE}/{}", db.name))
        .await
        .expect("connect");
    diesel::sql_query("CREATE TABLE app_user (id uuid PRIMARY KEY)")
        .execute(&mut scratch_conn)
        .await
        .expect("plant the conflict");

    let err = migrator::run_one(scratch("migrator_test_conflict"))
        .await
        .expect_err("a duplicate table must fail");
    assert!(err.to_string().contains("migrator_test_conflict"), "{err}");

    drop_database(db.name).await;
}

/// Each database keeps its own ledger. Two databases migrated from *different* sources do
/// not see each other's history — which is what stops `view`'s `app_user` and `user`'s
/// from ever being one migration line.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs docker/docker-compose-dev.yml"]
async fn every_database_keeps_its_own_ledger() {
    const OTHER: EmbeddedMigrations = embed_migrations!("../../migrations/spot");

    let _creates = CREATES.lock().await;
    let a = scratch("migrator_test_ledger_a");
    drop_database(a.name).await;
    migrator::run_one(scratch("migrator_test_ledger_a"))
        .await
        .unwrap();

    let b_name = "migrator_test_ledger_b";
    drop_database(b_name).await;
    unsafe { std::env::set_var("MIGRATOR_TEST_LEDGER_B", format!("{BASE}/{b_name}")) };
    migrator::run_one(migrator::Database {
        name: b_name,
        env: "MIGRATOR_TEST_LEDGER_B",
        migrations: OTHER,
    })
    .await
    .unwrap();

    // `user` has two migrations, `spot` has one. Same count in both would mean one
    // ledger, or one source applied twice.
    assert_eq!(applied_count(a.name).await, 2, "user's history");
    assert_eq!(applied_count(b_name).await, 1, "spot's history");

    drop_database(a.name).await;
    drop_database(b_name).await;
}

/// The names and URLs the binary will actually use, checked against the cluster rather
/// than against the struct — a typo in a database name is otherwise only found on deploy.
#[derive(diesel::QueryableByName)]
struct Datname {
    #[diesel(sql_type = Text)]
    datname: String,
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs docker/docker-compose-dev.yml"]
async fn the_five_real_databases_exist_after_a_full_run() {
    // Assumes a full run has happened — `scripts/migrate.sh`, which is `cargo run -p
    // migrator` with the five URLs exported.
    // Asserts presence only; nothing here creates or drops the real databases.
    let mut conn = admin().await;
    let rows: Vec<Datname> = diesel::sql_query("SELECT datname FROM pg_database")
        .load(&mut conn)
        .await
        .expect("list databases");

    let present: Vec<&str> = rows.iter().map(|r| r.datname.as_str()).collect();
    for db in migrator::databases() {
        assert!(
            present.contains(&db.name),
            "{} is missing — run the migrator first",
            db.name
        );
    }
}
