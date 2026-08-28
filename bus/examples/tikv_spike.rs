//! Step 0 of the TiKV rewrite: does an optimistic SurrealDB transaction actually
//! serialize two concurrent reserves for the same spot?
//!
//! Everything else in the rewrite depends on the answer. Today a double booking is
//! prevented by `bus::publish_expecting` asserting
//! `Nats-Expected-Last-Subject-Sequence` on the spot's subject — one writer wins,
//! the loser catches up and retries. Once TiKV is authoritative that CAS is gone,
//! and a plain `BEGIN … COMMIT` has to replace it.
//!
//! Reading surrealdb-core 3.2.4 says it cannot, on its own:
//!
//!   - `LockType::{Optimistic,Pessimistic}` both exist (`kvs/tr.rs:23`) and reach
//!     `TransactionOptions::new_pessimistic()` (`kvs/tikv/mod.rs:375`), but every
//!     one of the 479 production call sites passes `Optimistic`. The `Pessimistic`
//!     ones all sit past `ds.rs:5004`, where `#[cfg(test)]` opens.
//!   - `TikvConfig` (`kvs/tikv/cnf.rs`) has no lock-mode knob, and there is no
//!     `SURREAL_TIKV_*` variable for one.
//!   - `get_for_update` appears nowhere in the TiKV backend, so even a pessimistic
//!     transaction would take no locks on a plain `SELECT`.
//!
//! Optimistic conflict detection fires on WRITTEN keys. Two reserves for one spot
//! write two different booking records, so there is nothing to collide on — both
//! commit. This program is here to confirm that on a running cluster rather than
//! on paper, and to prove the mitigation: a bump of a shared per-spot counter
//! inside the same transaction, which is the standard optimistic-concurrency
//! version column.
//!
//! Run:
//!
//!     docker compose -f docker/docker-compose-dev.yml up -d
//!     cargo build --workspace --all-targets && ./target/debug/examples/tikv_spike
//!
//! Built with `--workspace`, never `-p bus` — see CLAUDE.md.
//!
//! Runs against the real `booking` database, so it exercises the actual
//! SCHEMAFULL tables rather than a synthetic copy. It cleans up the rows it
//! writes; every run uses fresh uuids, so a failed run leaves at most one spot
//! and two bookings behind.

use std::time::Duration;

use surrealdb::{Surreal, engine::remote::ws::Client};
use uuid::Uuid;

/// The shared SurrealDB from docker-compose-dev.yml, which is now on TiKV.
const ADDR: &str = "127.0.0.1:8000";

/// booking-service's database — where the `spot` and `booking` tables live.
const DB: &str = "booking";

/// How long each transaction sits between its reads and its write.
///
/// This is what makes the two genuinely overlap. Without it they can serialize by
/// luck — the first commits before the second begins — and the spike would report
/// a pass that means nothing.
const THINK: Duration = Duration::from_millis(300);

/// One reserve attempt, shaped like `BookingService::create_booking`: read the
/// spot, read what already blocks the slots, decide, then write.
///
/// `bump` is the whole question. With it off this is what a naive port of
/// `create_booking` to a transaction looks like; with it on it is the proposal.
async fn reserve(
    db: Surreal<Client>,
    spot_id: Uuid,
    booking_id: Uuid,
    bump: bool,
) -> Result<(), surrealdb::Error> {
    let tx = db.begin().await?;

    // 1. Read the spot. In the real path this is what prices the booking and
    //    supplies the CAS cursor.
    let _seq: Option<i64> = tx
        .query("SELECT VALUE bookings_seq FROM ONLY type::record('spot', $s)")
        .bind(("s", spot_id))
        .await?
        .take(0)?;

    // 2. Read what already blocks these slots.
    let _taken: Vec<String> = tx
        .query("SELECT VALUE status FROM booking WHERE spot_id = $s")
        .bind(("s", spot_id))
        .await?
        .take(0)?;

    // 3. `policy::availability::check` would run here. Both transactions have now
    //    read the same world and both believe the slot is free.
    tokio::time::sleep(THINK).await;

    // 4. Write the booking. Two different record ids — which is exactly why an
    //    optimistic transaction has nothing to detect.
    tx.query(
        "CREATE type::record('booking', $b) CONTENT {
             spot_id:    $s,
             owner_id:   $o,
             renter_id:  $r,
             booked:     { '2030-01-01': [{ start: '10:00', end: '11:00' }] },
             amount:     500,
             ends_at:    type::datetime('2030-01-01T11:00:00Z'),
             created_at: time::now()
         }",
    )
    .bind(("b", booking_id))
    .bind(("s", spot_id))
    .bind(("o", Uuid::now_v7()))
    .bind(("r", booking_id))
    .await?
    .check()?;

    if bump {
        // The mitigation. Both transactions write this one key, so TiKV has a
        // collision to detect and exactly one of them survives. `bookings_seq`
        // already exists for the CAS it is replacing — it stops being a stream
        // sequence and becomes a plain version counter.
        tx.query("UPDATE type::record('spot', $s) SET bookings_seq += 1")
            .bind(("s", spot_id))
            .await?
            .check()?;
    }

    tx.commit().await?;
    Ok(())
}

/// Fresh spot, two concurrent reserves, report who won.
///
/// Returns how many booking rows survived — 2 means the slot was sold twice.
async fn scenario(label: &str, bump: bool) -> Result<usize, Box<dyn std::error::Error>> {
    let spot_id = Uuid::now_v7();

    // Every SPOTS-owned column on this table is `option<>` (the two projectors
    // advance independently, so a partial row is the normal case), which is what
    // lets the seed be this small.
    let setup = shared::db::connect(ADDR, "root", "root", DB).await?;
    setup
        .query("UPSERT type::record('spot', $s) SET bookings_seq = 0")
        .bind(("s", spot_id))
        .await?
        .check()?;

    // Two independent sessions. `Surreal::begin` consumes its client, and these
    // must be in flight simultaneously, so one connection cannot serve both.
    let (a, b) = (
        shared::db::connect(ADDR, "root", "root", DB).await?,
        shared::db::connect(ADDR, "root", "root", DB).await?,
    );

    let (one, two) = tokio::join!(
        reserve(a, spot_id, Uuid::now_v7(), bump),
        reserve(b, spot_id, Uuid::now_v7(), bump),
    );

    println!("\n{label}");
    for (n, outcome) in [("txn 1", &one), ("txn 2", &two)] {
        match outcome {
            Ok(()) => println!("  {n}: committed"),
            Err(e) => {
                println!("  {n}: REFUSED");
                // Step 4 of the spike: the retry has to match on a type, not on a
                // string, so print enough to write that match against.
                println!("      Display: {e}");
                println!("      Debug:   {e:?}");
            }
        }
    }

    let rows: Option<i64> = setup
        .query("SELECT VALUE count() FROM booking WHERE spot_id = $s GROUP ALL")
        .bind(("s", spot_id))
        .await?
        .take(0)?;
    let rows = rows.unwrap_or(0) as usize;
    println!("  bookings on this spot: {rows}");

    // TiKV is authoritative and this is the real `booking` database, so unlike
    // the old disposable projection stores these rows would otherwise survive
    // every run and accumulate.
    setup
        .query("DELETE booking WHERE spot_id = $s")
        .bind(("s", spot_id))
        .await?
        .check()?;
    setup
        .query("DELETE type::record('spot', $s)")
        .bind(("s", spot_id))
        .await?
        .check()?;

    Ok(rows)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    shared::install_default_crypto_provider();

    println!("TiKV concurrency spike — SurrealDB {ADDR}");
    println!("Two concurrent reserves for one spot, {THINK:?} apart from read to write.");

    // Scenario A: confirm the hazard is real. A pass here is BAD news for the
    // naive port, which is the point of running it.
    let without = scenario("Scenario A — no version bump (naive transaction)", false).await?;

    // Scenario B: the proposal.
    let with = scenario(
        "Scenario B — with `UPDATE spot SET bookings_seq += 1`",
        true,
    )
    .await?;

    println!("\n─────────────────────────────────────────────");
    let hazard = without == 2;
    let fixed = with == 1;

    println!(
        "hazard is real (A wrote 2 bookings):        {}",
        if hazard { "YES" } else { "no" }
    );
    println!(
        "version bump serializes (B wrote 1):        {}",
        if fixed { "YES" } else { "NO" }
    );

    if !hazard {
        println!(
            "\n⚠  Scenario A did not double-book. Either the transactions did not\n\
             \x20  overlap (raise THINK), or this SurrealDB serializes more than its\n\
             \x20  source suggests. Do not treat B as proof until A reproduces."
        );
    }
    if !fixed {
        println!(
            "\n✗  The mitigation did not hold. The booking invariant has no\n\
             \x20  replacement — stop and revisit §4 of the plan before going further."
        );
    }
    if hazard && fixed {
        println!("\n✓  §4 confirmed: keep the version bump, drop nothing else.");
    }

    // Non-zero on a result the plan cannot proceed from.
    if !fixed {
        std::process::exit(1);
    }
    Ok(())
}
