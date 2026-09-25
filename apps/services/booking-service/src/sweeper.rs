//! Releases holds whose expiry has passed.
//!
//! The third event loop, beside `projector.rs` and `worker.rs` — driven by the clock
//! rather than by a stream, which is the only difference. It answers no request, so
//! it is not one of the `service/` types.
//!
//! This is what lets a `reserved` row mean "taken" with no qualification. The
//! alternative — every availability read also filtering on `hold_until` — leaves a
//! lapsed hold blocking a slot *forever* on a spot that sees no further events, and
//! puts the same invariant in three separate readers who each have to remember it.
//!
//! The cost of moving it here is that this task is load-bearing: with no read-time
//! filter behind it, a sweeper that stops means holds that never release. Hence
//! `run` below can only ever log and continue — nothing propagates out of the loop.

use std::time::Duration;

use diesel_async::AsyncConnection;
use diesel_async::scoped_futures::ScopedFutureExt;
use shared::{
    domain_models::booking::status,
    error::myerror::{MyError, MyResult},
    events::{
        Envelope, aggregate_id,
        booking::{BookingEvent, ReleaseReason},
        booking_subject,
    },
};

use crate::repository::booking_repository::BookingRepository;

const TICK: Duration = Duration::from_secs(60);

/// Cap per tick so one enormous backlog can't monopolise a tick or a publish batch.
/// Whatever is left is picked up on the next pass a minute later.
const MAX_PER_SWEEP: usize = 200;

/// Sweeps forever. `tokio::spawn` is the caller's, like the projectors' and the
/// worker's — this loop is no more special than theirs.
pub async fn run(db: shared::db::Db) {
    let mut ticker = tokio::time::interval(TICK);
    loop {
        ticker.tick().await;
        // Deliberately swallowed. A failed tick is retried a minute later; a
        // propagated error would end the task, and nothing else frees a hold.
        if let Err(e) = sweep(&db).await {
            tracing::error!(error = %e, "sweep failed; retrying next tick");
        }
    }
}

async fn sweep(db: &shared::db::Db) -> MyResult<()> {
    let mut read = shared::db::conn(db).await?;
    let lapsed = BookingRepository::lapsed_holds(&mut read, MAX_PER_SWEEP).await?;
    drop(read);
    if lapsed.is_empty() {
        return Ok(());
    }
    tracing::info!(count = lapsed.len(), "releasing lapsed holds");

    for hold in lapsed {
        let (booking_id, spot_id) = (hold.id, hold.spot_id);

        let event = BookingEvent::Released {
            booking_id,
            reason: ReleaseReason::Expired,
        };
        // One transaction per hold. `?` inside rolls back and the loop moves on — the
        // hold stays `reserved` with a past `hold_until`, so the next tick a minute
        // from now picks it up again. Nothing is lost by failing here.
        //
        // Each step keeps its own message: "could not release" and "could not set
        // version" are different faults, and collapsing them into one would make the
        // log say only that a sweep failed.
        let mut conn = match shared::db::conn(db).await {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(booking = %booking_id, error = %e, "no connection");
                continue;
            }
        };

        let released = conn
            .transaction::<_, MyError, _>(|conn| {
                let event = event.clone();
                async move {
                    let version =
                        shared::next_version!(conn, shared::schema::booking::booking, &booking_id)
                            .inspect_err(|e| {
                                tracing::error!(booking = %booking_id, error = %e, "could not read version")
                            })?;

                    // Scoped to `reserved`, which is what stops this undoing a payment
                    // that landed in the same instant — the sweeper and `confirm_paid`
                    // genuinely race, and the guard is the whole answer to it.
                    let released = BookingRepository::transition(
                        conn,
                        booking_id,
                        status::RELEASED,
                        &[status::RESERVED],
                        Some(ReleaseReason::Expired.as_str()),
                        None,
                    )
                    .await
                    .inspect_err(|e| {
                        tracing::error!(booking = %booking_id, error = %e, "could not release")
                    })?;

                    // Somebody else already moved this booking — the other replica's
                    // sweeper inside the same 60s tick, or `confirm_paid` winning the
                    // race above. Nothing changed here, so there is nothing to number
                    // and nothing to publish.
                    //
                    // Bumping the version anyway is what this used to do, and it was the
                    // expensive half of a silent bug: the event that carried the bumped
                    // version was collapsed by the deterministic id two lines below,
                    // leaving a version committed with no event behind it. Every
                    // projector then read that hole as a gap and parked the booking for
                    // the full escape-hatch budget, on every replay, for ever.
                    if !released.applied() {
                        return Ok(());
                    }

                    shared::set_version!(conn, "booking", shared::schema::booking::booking, &booking_id, version)
                        .inspect_err(|e| {
                            tracing::error!(booking = %booking_id, error = %e, "could not set version")
                        })?;

                    // `actor_id: None` — nobody requested this, the clock did.
                    //
                    // A plain v7 event id. This used to be `v5("expire:{booking_id}")`, so
                    // that two instances sweeping the same booking inside the stream's
                    // duplicate window collapsed to one event. The guard above is what
                    // does that now, and it does it better: the second sweeper never
                    // reaches this line, so there is no second event to collapse and no
                    // version burnt producing one. Suppressing the event downstream while
                    // the version had already been committed upstream was the bug.
                    let envelope =
                        Envelope::new(event, None, aggregate_id("booking", &booking_id), version);

                    // No compare-and-swap: a release only ever frees slots, so it can't
                    // lose a race in a way that matters. Applying it is guarded on the
                    // booking still being 'reserved'.
                    bus::outbox::enqueue(conn, &booking_subject(&spot_id), &envelope)
                        .await
                        .inspect_err(|e| {
                            tracing::error!(booking = %booking_id, error = %e, "could not enqueue release")
                        })?;

                    Ok(())
                }
                .scope_boxed()
            })
            .await;

        if released.is_err() {
            // Already logged with its specific cause above; this only records that the
            // hold survives to the next tick.
            tracing::warn!(booking = %booking_id, "hold not released; next tick retries");
        }
    }
    Ok(())
}

/// The sweeper against a real database, because the property under test is a race that
/// only the database can arbitrate.
///
/// ```sh
/// docker compose -f docker/docker-compose-dev.yml up -d yugabyte
/// cargo test --workspace -- --ignored
/// ```
#[cfg(test)]
mod live_tests {
    use chrono::{TimeDelta, Utc};
    use diesel::prelude::*;
    use diesel_async::RunQueryDsl;
    use shared::domain_models::booking::Booking;
    use shared::schema::booking::booking;
    use uuid::Uuid;

    use super::*;
    use crate::repository::booking_repository::BookingRepository;

    async fn db() -> shared::db::Db {
        // SAFETY: tests in one binary share an environment and every caller sets the
        // same value.
        unsafe {
            std::env::set_var(
                "BOOKING_DATABASE_URL",
                "postgres://yugabyte@127.0.0.1:5433/booking",
            )
        };
        migrator::ensure("booking").await.expect("migrations apply");
        shared::db::connect("postgres://yugabyte@127.0.0.1:5433/booking")
            .await
            .expect("dev yugabyte on :5433")
    }

    /// How many outbox rows mention this booking. The payload is the encoded envelope,
    /// so the id appears in it.
    async fn pending_for(pool: &shared::db::Db, id: Uuid) -> i64 {
        let mut c = shared::db::conn(pool).await.unwrap();
        bus::schema::_outbox::table
            .filter(bus::schema::_outbox::payload.like(format!("%{id}%")))
            .count()
            .get_result::<i64>(&mut *c)
            .await
            .unwrap()
    }

    /// A hold already past its expiry, so `lapsed_holds` picks it up.
    fn a_lapsed_hold(id: Uuid) -> Booking {
        Booking {
            id,
            version: 1,
            spot_id: Uuid::now_v7(),
            host_id: Uuid::now_v7(),
            renter_id: Uuid::now_v7(),
            booked: Default::default(),
            license_plate: "1-ABC-123".into(),
            amount: 500,
            status: status::RESERVED.to_string(),
            hold_until: Some(Utc::now() - TimeDelta::minutes(1)),
            release_reason: None,
            cancel_reason: None,
            ends_at: Utc::now() + TimeDelta::hours(1),
            rating: None,
            created_at: Utc::now(),
        }
    }

    /// **The regression test for the whole class.**
    ///
    /// Two sweepers inside one 60s tick — two replicas, which is the normal deployment —
    /// both see the same lapsed hold. One releases it; the other's guard matches nothing.
    ///
    /// The loser used to mint version 2, commit it with `set_version!`, and enqueue a
    /// `Released` under `v5("expire:{booking_id}")`. That id collided with the winner's,
    /// so `ON CONFLICT DO UPDATE` overwrote the winner's pending row and only one event
    /// survived — at the wrong version, with the other never published. The booking sat
    /// at version 2 with version 1's event in the stream, and every projector that met it
    /// parked the booking for the full escape-hatch budget, on every replay, for ever.
    ///
    /// Asserting the version is what actually catches it. The row's *status* was always
    /// right; it was the numbering that broke.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "needs docker/docker-compose-dev.yml"]
    async fn a_second_sweep_of_one_hold_publishes_nothing_and_burns_no_version() {
        let pool = db().await;

        // A batch, not one booking, and that is what makes this reliable rather than
        // flaky. The window is narrow — both sweepers must run `lapsed_holds` before
        // either commits — and with a single booking it opened about one run in four.
        // Two concurrent sweeps over a batch give one independent chance to race per
        // booking, and the assertions are ones a *non*-raced booking also satisfies, so
        // widening the net cannot produce a false failure. It can only find the bug.
        let ids: Vec<Uuid> = (0..12).map(|_| Uuid::now_v7()).collect();
        {
            let mut c = shared::db::conn(&pool).await.unwrap();
            for id in &ids {
                BookingRepository::upsert(&mut c, a_lapsed_hold(*id))
                    .await
                    .unwrap();
            }
        }

        // Real tasks, not `tokio::join!`. Joined futures interleave only at await points
        // and reliably let the first sweep finish first, which closes the very window
        // under test — `lapsed_holds` then returns nothing to the second.
        let (one, two) = (pool.clone(), pool.clone());
        let (a, b) = tokio::join!(
            tokio::spawn(async move { sweep(&one).await }),
            tokio::spawn(async move { sweep(&two).await })
        );
        a.unwrap().expect("first sweep");
        b.unwrap().expect("second sweep");

        let mut c = shared::db::conn(&pool).await.unwrap();
        for id in &ids {
            let got: Booking = booking::table
                .find(id)
                .select(Booking::as_select())
                .first(&mut *c)
                .await
                .unwrap();

            assert_eq!(got.status, status::RELEASED);
            assert_eq!(
                got.version, 2,
                "booking {id} reached version {} — a sweeper whose guard matched nothing \
                 burnt a version, so the event for the version below it has nothing \
                 behind it and every projector parks on the gap",
                got.version
            );
            assert_eq!(
                pending_for(&pool, *id).await,
                1,
                "booking {id} enqueued more than one release"
            );
        }

        // The deterministic half, independent of whether any booking actually raced: the
        // guard must *report* a refusal rather than just decline to write. Everything
        // above rests on this being the signal the sweeper reads.
        let refused = BookingRepository::transition(
            &mut c,
            ids[0],
            status::RELEASED,
            &[status::RESERVED],
            Some(ReleaseReason::Expired.as_str()),
            None,
        )
        .await
        .unwrap();
        assert_eq!(
            refused,
            shared::db::Changed::No,
            "the losing sweeper must be told its guard matched nothing"
        );

        // The outbox rows too, not just the bookings. Nothing drains `_outbox` in a test
        // run, so leaving them accumulates a table that `pending_for` then scans with
        // `LIKE` — slower on every run, without bound.
        for id in &ids {
            diesel::delete(booking::table.find(id))
                .execute(&mut *c)
                .await
                .unwrap();
            diesel::delete(
                bus::schema::_outbox::table
                    .filter(bus::schema::_outbox::payload.like(format!("%{id}%"))),
            )
            .execute(&mut *c)
            .await
            .unwrap();
        }
    }
}
