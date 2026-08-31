//! Checks the leader election against a live database.
//!
//! The property that matters is boring to state and easy to get wrong: **at most
//! one holder at a time, and a dead holder's lease is takeable**. Everything in
//! step 3 — which instance projects, which one relays — rests on it.
//!
//! An `#[ignore]`d unit test would be the usual home for this, but the mechanism *is*
//! the database's: a single `INSERT … ON CONFLICT DO UPDATE … WHERE` whose guard
//! decides whether the update branch fires. There is nothing left to test once the
//! database is mocked out, so it runs against the real stack.
//!
//!     docker compose -f docker/docker-compose-dev.yml up -d
//!     cargo build --workspace --all-targets && ./target/debug/examples/lease_check
//!
//! ## What changed with the database
//!
//! The contended-start check below used to be described as "the write-write conflict
//! doing the work, not the lease logic" — two transactions writing one row, one
//! refused by TiKV. **That is no longer how it works.** Under Read Committed the loser
//! is not refused: it blocks until the winner commits, then re-evaluates its `WHERE`
//! against the winner's fresh row and declines because the lease is held and unexpired.
//!
//! The outcome is the same and the property is the same, so this file still earns its
//! keep — but it is now checking the statement's guard rather than the store's
//! conflict detection, which is why `acquire` returning `Ok(false)` is the ordinary
//! loss and `Err` has become rare rather than expected.
//!
//! Built with `--workspace`, never `-p bus` — see CLAUDE.md.

use bus::lease;

const URL: &str = "postgres://yugabyte@127.0.0.1:5433/booking";
const NAME: &str = "leader-selftest";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    shared::install_default_crypto_provider();

    let a = shared::db::connect(URL).await?;
    let b = shared::db::connect(URL).await?;
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

    // The one that matters: N instances starting at once must not all believe they
    // won. See the module doc above for why this is now the statement's `WHERE` doing
    // the work rather than a write-write conflict.
    println!("\ncontended start (8 instances, one free lease)");
    lease::release(&b, NAME, &bob).await?;
    let mut set = tokio::task::JoinSet::new();
    for i in 0..8 {
        set.spawn(async move {
            let db = shared::db::connect(URL).await.ok()?;
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
