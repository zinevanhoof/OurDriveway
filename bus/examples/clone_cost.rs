//! `Surreal::clone` against `Arc::clone`.
//!
//! `shared::db::begin` is `db.clone().begin()`, so every write and every projected
//! event pays for one `Surreal::clone`. Everything else in the codebase holds an
//! `Arc<Surreal<Client>>` and clones the `Arc`. This is the difference between those
//! two, measured rather than assumed.
//!
//! They are not variations on the same thing:
//!
//!   - `Arc::clone` is a refcount bump. Same session, nothing on the wire.
//!   - `Surreal::clone` mints a session id and has the engine **replay** every
//!     `replayable()` command onto it (`engine/remote/mod.rs:355`) — for a remote
//!     connection built by `shared::db::connect` that is `Attach`, `Signin`, `Use`.
//!     The call returns immediately (`lib.rs:340`, a `try_send` that is never
//!     awaited); the server pays for the replay ahead of whatever the clone asks
//!     for next.
//!
//! ## Measured 2026-08-27, three ways, 100 iterations each
//!
//! Three configurations, because two variables had to be separated: the WS session
//! replay, and the storage backend under it.
//!
//! | client / server | backend | `Surreal::clone` | `Arc::clone` | completed |
//! |---|---|---|---|---|
//! | 3.2.4, embedded `Mem` | — (no socket) | 1.7ms | 1.6ms | 100/100 |
//! | 3.2.4 / 3.2.4 | `memory` | **24.4ms** | **635µs** | 100/100 |
//! | 3.2.4 / 3.2.4 | `tikv://pd:2379` | — | — | **hangs on iteration 1** |
//! | 3.2.1 / 3.2.1 | `tikv://pd:2379` | — | — | **hangs on iteration 1** |
//!
//! The 3.2.1 row is there because "is this a regression?" is the first thing anyone
//! will ask. It is not — 3.2.1 hangs identically, on a wiped datastore.
//!
//! Read across the rows and the two findings come apart cleanly:
//!
//!   - **The cost is the WS replay, not the backend.** 24.4ms against 635µs is ~38x,
//!     and it is the same order over `memory` as over TiKV, while the embedded engine
//!     — which has no socket and no `Signin` in its replay log — shows none of it. So
//!     the bill is the replayed root sign-in (an Argon2 verify) that every cloned
//!     session drags behind it, and the fix for *that* is a pool of pre-authenticated
//!     sessions, not anything about cloning.
//!
//!   - **The hang is TiKV-specific, and not new in 3.2.4.** Same image, same protocol,
//!     same sign-in, same three statements — only the datastore differs, and only TiKV
//!     wedges. One or two cloned queries answer and then the connection goes silent for
//!     good: no error, no timeout, the process idle at 0% CPU. Observed for 5m40s in
//!     one run.
//!
//!     The server is **not** the casualty. While the client is wedged, another
//!     connection reads the rows iteration 0 committed in 2ms, and still does after the
//!     client is killed. Nothing is locked and nothing is corrupt — one request on a
//!     cloned session is simply never answered. The identical SQL over HTTP `/sql`
//!     against the same TiKV runs in 60ms.
//!
//! ## Two traps this test fell into, both worth not repeating
//!
//! **Datastore state does not survive a version change.** The first 3.2.1 run reused a
//! datastore 3.2.4 had written and every read came back `Failed to get table` — an
//! `Internal` error that says nothing about versions and persists after the client
//! dies. It made a contaminated run look like a clean reproduction. Any version move
//! here needs `down -v`.
//!
//! **3.2.4 creates a namespace implicitly on first use; 3.2.1 does not**, answering
//! `The namespace 'x' does not exist`. This file therefore issues `DEFINE NAMESPACE`
//! and `DEFINE DATABASE` explicitly, so the two versions are running the same program.
//!
//! Note what this does **not** show. Both loops here run a plain `query`; the services
//! only ever clone in `shared::db::begin`, which is `clone().begin()` — a transaction,
//! not a query. `clone().query()` appears nowhere in the codebase. A separate bench of
//! `clone().begin()` + `commit()` ran 10/10 clean at 28ms against the same TiKV. So
//! this reproduces a real client bug reliably, and does not by itself convict the
//! services.
//!
//! Run:
//!
//!     docker compose -f docker/docker-compose-dev.yml up -d
//!     cargo build --workspace --all-targets && ./target/debug/examples/clone_cost
//!
//! And the control that separates client from backend:
//!
//!     docker run -d --name surrealdb-local -p 8001:8000 surrealdb/surrealdb:v3.2.4 \
//!         start --log=info --user=root --pass=root memory
//!     ./target/debug/examples/clone_cost 127.0.0.1:8001
//!
//! Built with `--workspace`, never `-p bus` — see CLAUDE.md.

use std::sync::Arc;
use std::time::Instant;

use surrealdb::{
    Connection, Surreal,
    // `engine::local::Mem` — the memory half is commented out in `main`; put this back
    // with it.
    engine::remote::ws::Ws,
    opt::auth::Root,
};

/// The shared SurrealDB from docker-compose-dev.yml, on TiKV.
///
/// Override with argv[1] to point at a different one — the reason that exists is to
/// separate the WS client's session replay from the storage backend underneath it:
///
///     docker run -d --name surrealdb-local -p 8001:8000 surrealdb/surrealdb:v3.2.4 \
///         start --log=info --user=root --pass=root memory
///     ./target/debug/examples/clone_cost 127.0.0.1:8001
///
/// Same image, same protocol, same sign-in — only `memory` instead of `tikv://pd:2379`.
const ADDR: &str = "127.0.0.1:8000";

/// Its own namespace, not one of the services'. This writes `person` rows and
/// `likes` edges by the hundred and drops the lot at the end.
const NS: &str = "clone_cost";

const ITERATIONS: usize = 100;

/// Three statements, one of them a `RELATE`, so each iteration is real work rather
/// than a `RETURN 1` the engine can answer without touching storage.
const WORK: &str = "
    LET $one = CREATE ONLY person;
    LET $two = CREATE ONLY person;
    RELATE $one->likes->$two;
";

/// Runs `WORK` `ITERATIONS` times twice — once cloning the `Surreal`, once cloning an
/// `Arc` around it — and prints both.
///
/// Generic over the engine so the memory and remote cases are the same code. They have
/// to be: a difference between the two rows is only meaningful if nothing else differs.
///
/// Takes the connection **by value and hands it back**, so the caller passes the base
/// session itself rather than `remote.clone()`. That distinction is the whole point:
/// the services only ever clone the session `connect()` returned, so a test that cloned
/// a clone would be measuring a generation deeper than anything they do.
async fn compare<C: Connection>(label: &str, db: Surreal<C>) -> surrealdb::Result<Surreal<C>> {
    let started = Instant::now();
    for i in 0..ITERATIONS {
        let cloned = db.clone();
        // Printed per iteration, and before the await rather than after. This loop can
        // stop answering entirely — on the remote engine it has hung on iteration 0
        // with the process idle at 0% CPU — and a counter that only prints on success
        // cannot tell "hung on the first" from "never started".
        eprint!("\r  Surreal::clone {i:>3}/{ITERATIONS}");
        cloned.query(WORK).await?;
    }
    eprint!("\r");
    let surreal_clone = started.elapsed();

    let arced = Arc::new(db);
    let started = Instant::now();
    for _ in 0..ITERATIONS {
        let cloned_arc = Arc::clone(&arced);
        cloned_arc.query(WORK).await?;
    }
    let arc_clone = started.elapsed();

    println!("\n{label}  ({ITERATIONS} iterations)");
    println!("  Surreal::clone  {surreal_clone:>12.1?}   {:>8.1?} each", surreal_clone / ITERATIONS as u32);
    println!("  Arc::clone      {arc_clone:>12.1?}   {:>8.1?} each", arc_clone / ITERATIONS as u32);

    // Every `cloned_arc` was dropped at the end of its iteration, so this is the last
    // strong reference and the unwrap cannot fail.
    Ok(Arc::into_inner(arced).expect("sole owner"))
}

#[tokio::main]
async fn main() -> surrealdb::Result<()> {
    shared::install_default_crypto_provider();

    // Embedded, commented out rather than deleted — it has already answered its
    // question and it is the control worth re-running if the remote numbers ever
    // change. No socket, and no `Signin` in its replay log because there is nothing to
    // sign in to, which is exactly why it does not reproduce the remote hang.
    //
    // Measured 2026-08-27, 100 iterations: `Surreal::clone` 1.7ms each,
    // `Arc::clone` 1.6ms each. Restore the `local::Mem` import with it.
    //
    // let mem = Surreal::new::<Mem>(()).await?;
    // mem.use_ns("ns").use_db("db").await?;
    // compare("memory engine", mem).await?;

    // Remote: the arrangement every service actually runs on. Signed in as root, so
    // the replay a clone carries includes an Argon2 verify.
    //
    // Each setup step announces itself. The remote half has hung before producing any
    // output at all, and "no rows written" is equally consistent with a hang in
    // `connect`, in `signin`, in `use_ns`, or on the first cloned query — four very
    // different bugs. Narrowing that by hand cost a run.
    let addr = std::env::args().nth(1).unwrap_or_else(|| ADDR.to_string());
    eprintln!("remote: connecting to {addr}");
    let remote = Surreal::new::<Ws>(addr.as_str()).await?;
    eprintln!("remote: signing in");
    remote
        .signin(Root {
            username: "root".to_string(),
            password: "root".to_string(),
        })
        .await?;
    // Defined explicitly, not left to `use_ns` to conjure. 3.2.4 creates a namespace
    // implicitly on first use and 3.2.1 answers `The namespace 'x' does not exist`, so
    // a test that relied on the implicit path would be comparing two different things
    // across versions. Cost a run to find out.
    eprintln!("remote: define ns/db");
    remote
        .query(format!("DEFINE NAMESPACE IF NOT EXISTS {NS}"))
        .await?
        .check()?;
    remote.use_ns(NS).await?;
    remote.query("DEFINE DATABASE IF NOT EXISTS bench").await?.check()?;
    eprintln!("remote: use ns/db");
    remote.use_db("bench").await?;
    eprintln!("remote: ready");
    // `remote`, not `remote.clone()`. The loop inside clones the base session, which is
    // the only thing `shared::db::begin` ever clones.
    let remote = compare(&format!("remote ws — {addr}"), remote).await?;

    // The rows are this program's alone, so the whole namespace goes.
    remote.query(format!("REMOVE NAMESPACE IF EXISTS {NS}")).await?.check()?;

    Ok(())
}
