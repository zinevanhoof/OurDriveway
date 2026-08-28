//! Checks the leader election against a live database.
//!
//! The property that matters is boring to state and easy to get wrong: **at most
//! one holder at a time, and a dead holder's lease is takeable**. Everything in
//! step 3 — which instance projects, which one relays — rests on it.
//!
//! An `#[ignore]`d unit test would be the usual home for this, but the mechanism
//! *is* the database's conflict detection: two transactions writing one row, one
//! refused. There is nothing left to test once the database is mocked out, so it
//! lives here beside `tikv_spike.rs` and runs against the real stack.
//!
//!     docker compose -f docker/docker-compose-dev.yml up -d
//!     cargo build --workspace --all-targets && ./target/debug/examples/lease_check
//!
//! Built with `--workspace`, never `-p bus` — see CLAUDE.md.

use bus::lease;

const ADDR: &str = "127.0.0.1:8000";
const DB: &str = "booking";
const NAME: &str = "leader-selftest";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    shared::install_default_crypto_provider();

    let a = shared::db::connect(ADDR, "root", "root", DB).await?;
    let b = shared::db::connect(ADDR, "root", "root", DB).await?;
    let (alice, bob) = ("alice".to_string(), "bob".to_string());

    // Never inherit a lease from a previous run.
    lease::release(&a, NAME, &alice).await.ok();
    lease::release(&a, NAME, &bob).await.ok();

    let mut failures = 0;
    let mut check = |label: &str, got: bool, want: bool| {
        let ok = got == want;
        println!(
            "  {} {label}: {got}",
            if ok { "✓" } else { "✗" }
        );
        if !ok {
            failures += 1;
        }
    };

    println!("\nuncontended");
    check("alice takes a free lease", lease::acquire(&a, NAME, &alice).await?, true);
    check("alice renews her own", lease::acquire(&a, NAME, &alice).await?, true);
    check("bob refused while alice holds", lease::acquire(&b, NAME, &bob).await?, false);

    println!("\nhandover after release");
    lease::release(&a, NAME, &alice).await?;
    check("bob takes the released lease", lease::acquire(&b, NAME, &bob).await?, true);
    check("alice now refused", lease::acquire(&a, NAME, &alice).await?, false);

    // A losing racer must not be able to evict the winner.
    println!("\nrelease is scoped to the holder");
    lease::release(&a, NAME, &alice).await?;
    check("alice's release did not evict bob", lease::acquire(&a, NAME, &alice).await?, false);
    check("bob still holds", lease::acquire(&b, NAME, &bob).await?, true);

    // The one that matters: N instances starting at once must not all believe
    // they won. This is the write-write conflict doing the work, not the lease
    // logic — see the module docs in bus/src/lease.rs.
    println!("\ncontended start (8 instances, one free lease)");
    lease::release(&b, NAME, &bob).await?;
    let mut set = tokio::task::JoinSet::new();
    for i in 0..8 {
        set.spawn(async move {
            let db = shared::db::connect(ADDR, "root", "root", DB).await.ok()?;
            // `Ok(false)` is a clean loss; `Err` is losing the write race, which is
            // also a loss. Neither may be reported as a win.
            lease::acquire(&db, NAME, &format!("instance-{i}"))
                .await
                .ok()
                .filter(|held| *held)
                .map(|_| i)
        });
    }
    let mut winners = vec![];
    while let Some(res) = set.join_next().await {
        if let Ok(Some(i)) = res {
            winners.push(i);
        }
    }
    println!("  winners: {winners:?}");
    check("exactly one instance won", winners.len() == 1, true);

    lease::release(&a, NAME, &format!("instance-{}", winners.first().copied().unwrap_or(0)))
        .await
        .ok();

    println!("\n─────────────────────────────────────────────");
    if failures == 0 {
        println!("✓  lease holds: one leader at a time, handover works");
        Ok(())
    } else {
        println!("✗  {failures} check(s) failed — do NOT rely on the election");
        std::process::exit(1);
    }
}
