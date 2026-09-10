//! One repository per table, each holding the SQL for the statements this service
//! actually issues. There is no shared `Repository` trait behind them and nothing is
//! generated: what a method does is the string in front of you, with no `format!` and
//! no consts spliced in from the domain models.
//!
//! Two tables, because this service consumes two streams: its own `booking` rows, and
//! a mirror of the spots those bookings are made against.
//!
//! The mirror carries no copy of what is booked. Which slots are taken is
//! `BookingRepository::taken_for_spot`, a query over the rows themselves.
//!
//! Reads are `Selectable`. Writes bind the struct whole through `Insertable` and
//! `AsChangeset` — what `CONTENT $row` did, and what sqlx could not.
//!
//! The two tables differ in one way that still matters: `booking` is written whole,
//! since every column is BOOKINGS-owned, while `spot` is a mirror whose SPOTS writes
//! `COALESCE` each column so a partial event leaves the rest alone. That difference
//! used to be sharper — the mirror also carried `bookings_seq`, a column no SPOTS
//! event knew about — and that column is gone; see `SpotMirrorRepository`.

pub mod booking_repository;
pub mod spot_mirror_repository;

/// Round-trips both tables through a real YugabyteDB, and proves the reserve path
/// cannot double-book.
///
/// `#[ignore]`d — needs the dev cluster on :5433, and CI runs
/// `cargo test --workspace` with no database:
///
/// ```sh
/// docker compose -f docker/docker-compose-dev.yml up -d yugabyte
/// cargo test --workspace -- --ignored
/// ```
///
/// `two_racing_reserves_cannot_double_book` is the one that has to exist. Everything
/// else here checks that a statement does what it says; that one checks that the
/// **database behaves the way the whole reserve design assumes it does** — and the
/// assumption is version-dependent, so it is not something to take on trust. Its
/// negative control runs beside it, because a concurrency test that cannot fail is
/// worse than none.
#[cfg(test)]
mod live_tests {
    use std::collections::HashMap;

    use chrono::{DateTime, TimeDelta, Utc};
    use diesel::prelude::*;
    use diesel_async::scoped_futures::ScopedFutureExt;
    use diesel_async::{AsyncConnection, RunQueryDsl};
    use shared::domain_models::booking::{Booking, SpotMirrorPatch, status};
    use shared::general_models::booking::Booked;
    use shared::general_models::spot::{Availability, TimeSlot, WeeklyAvailability};
    use shared::schema::booking::{booking, spot};
    use uuid::Uuid;

    use super::booking_repository::BookingRepository;
    use super::spot_mirror_repository::SpotMirrorRepository;

    /// Connects and migrates, so a running container is the only prerequisite.
    async fn db() -> shared::db::Db {
        // SAFETY: tests in one binary share an environment and every caller sets the
        // same value.
        unsafe {
            std::env::set_var(
                "BOOKING_DATABASE_URL",
                "postgres://yugabyte@127.0.0.1:5433/booking",
            )
        };
        // Through the migrator rather than a second copy of the wiring: that crate is
        // the only thing that migrates in dev and production, so a test cannot drift
        // from what actually gets applied. It creates the database if missing.
        //
        // `ensure` and not `run_one`: test threads run in parallel and would otherwise
        // all try to apply a new migration at once, which YugabyteDB refuses rather than
        // serialises. See the note on that function.
        migrator::ensure("booking").await.expect("migrations apply");

        shared::db::connect("postgres://yugabyte@127.0.0.1:5433/booking")
            .await
            .expect("dev yugabyte on :5433 — see this module's docs")
    }

    /// One connection for a test to pass around: repositories take a connection, not a
    /// pool.
    async fn conn(
        db: &shared::db::Db,
    ) -> diesel_async::pooled_connection::bb8::PooledConnection<'_, diesel_async::AsyncPgConnection>
    {
        shared::db::conn(db).await.expect("a connection")
    }

    fn slots() -> Booked {
        HashMap::from([(
            "2026-08-19".to_string(),
            vec![TimeSlot {
                start: "09:00".to_string(),
                end: "11:00".to_string(),
            }],
        )])
        .into()
    }

    fn availability() -> Availability {
        Availability {
            weekly: WeeklyAvailability {
                monday: vec![],
                tuesday: vec![],
                wednesday: vec![TimeSlot {
                    start: "08:00".to_string(),
                    end: "18:00".to_string(),
                }],
                thursday: vec![],
                friday: vec![],
                saturday: vec![],
                sunday: vec![],
            },
            single: HashMap::new(),
        }
    }

    fn a_booking(id: Uuid, spot_id: Uuid, ends_at: DateTime<Utc>) -> Booking {
        Booking {
            id,
            version: 1,
            spot_id,
            host_id: Uuid::now_v7(),
            renter_id: Uuid::now_v7(),
            booked: slots(),
            amount: 500,
            status: status::RESERVED.to_string(),
            hold_until: Some(Utc::now() + TimeDelta::minutes(10)),
            release_reason: None,
            cancel_reason: None,
            ends_at,
            rating: None,
            created_at: Utc::now(),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn a_booking_round_trips_and_transitions_are_guarded() {
        let pool = db().await;
        let db = &mut *conn(&pool).await;

        let (id, spot_id) = (Uuid::now_v7(), Uuid::now_v7());
        // Comfortably in the future: `taken_for_spot` floors on `ends_at > now`, so a
        // booking that already ended would correctly not come back.
        let row = a_booking(id, spot_id, Utc::now() + TimeDelta::hours(2));

        BookingRepository::upsert(db, row.clone()).await.unwrap();

        let got = BookingRepository::find_by_id(db, id)
            .await
            .unwrap()
            .expect("upserted row");
        assert_eq!(got.id, id);
        assert_eq!(got.version, 1);
        assert_eq!(got.booked, slots(), "the jsonb column must round-trip");
        assert_eq!(got.amount, 500);
        assert_eq!(got.rating, None);
        assert!(got.hold_until.is_some());

        BookingRepository::transition(db, id, status::CONFIRMED, &[status::RESERVED], None, None)
            .await
            .unwrap();

        let got = BookingRepository::find_by_id(db, id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.status, status::CONFIRMED);
        assert_eq!(got.hold_until, None, "every transition ends the hold");
        assert_eq!(
            got.booked,
            slots(),
            "a transition must not disturb the rest"
        );

        // Redelivery: already out of `reserved`, so the guard refuses and the row is
        // untouched — this is what stops a lapsed-hold event undoing a confirmation
        // that raced it. Matching nothing is a no-op, not an error.
        BookingRepository::transition(
            db,
            id,
            status::RELEASED,
            &[status::RESERVED],
            Some("expired"),
            None,
        )
        .await
        .unwrap();
        let got = BookingRepository::find_by_id(db, id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.status, status::CONFIRMED, "the guard must have held");
        assert_eq!(got.release_reason, None);

        // What blocks the spot, which is what `spot.booked` used to cache.
        let taken = BookingRepository::taken_for_spot(db, &spot_id, Utc::now())
            .await
            .unwrap();
        assert_eq!(taken, slots(), "a confirmed booking blocks its slots");

        // Released and cancelled free the slots again.
        BookingRepository::transition(db, id, status::CANCELLED, &[status::CONFIRMED], None, None)
            .await
            .unwrap();
        let taken = BookingRepository::taken_for_spot(db, &spot_id, Utc::now())
            .await
            .unwrap();
        assert!(taken.is_empty(), "a cancelled booking blocks nothing");

        diesel::delete(booking::table.find(id))
            .execute(&mut *conn(&pool).await)
            .await
            .unwrap();
    }

    /// A partial SPOTS event must leave every column it does not name alone, in either
    /// arrival order — which on this table is the normal case, because the streams
    /// expire and a `SpotUpdated` can arrive for a spot whose `SpotCreated` aged out.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn a_partial_spots_write_leaves_unnamed_columns_alone() {
        let pool = db().await;
        let db = &mut *conn(&pool).await;
        let id = Uuid::now_v7();
        let host_id = Uuid::now_v7();

        SpotMirrorRepository::merge(
            db,
            id,
            SpotMirrorPatch {
                host_id: Some(host_id),
                price_per_hour: Some(700),
                availability: Some(availability()),
                timezone: Some("Europe/Brussels".to_string()),
                active: Some(true),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        let got = SpotMirrorRepository::find_by_id(db, id)
            .await
            .unwrap()
            .expect("merge created it");
        assert_eq!(got.host_id, Some(host_id));
        assert_eq!(got.price_per_hour, Some(700));
        assert!(got.bookable().is_some(), "the mirror knows enough now");

        // A later partial edit — the live switch — must leave everything else alone.
        SpotMirrorRepository::merge(
            db,
            id,
            SpotMirrorPatch {
                active: Some(false),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let got = SpotMirrorRepository::find_by_id(db, id)
            .await
            .unwrap()
            .unwrap();
        assert!(!got.active);
        assert!(!got.deleted, "the live switch must not delete");
        assert_eq!(got.timezone.as_deref(), Some("Europe/Brussels"));
        assert_eq!(got.price_per_hour, Some(700), "COALESCE must hold");
        assert!(got.bookable().is_some());

        // A row created by an edit alone — the expired-history case — must fail
        // closed rather than being bookable against nothing.
        let orphan = Uuid::now_v7();
        SpotMirrorRepository::merge(
            db,
            orphan,
            SpotMirrorPatch {
                active: Some(true),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let got = SpotMirrorRepository::find_by_id(db, orphan)
            .await
            .unwrap()
            .unwrap();
        assert!(
            got.bookable().is_none(),
            "a half-built mirror must fail closed"
        );

        diesel::delete(spot::table.filter(spot::id.eq_any([id, orphan])))
            .execute(&mut *conn(&pool).await)
            .await
            .unwrap();
    }

    /// One reserve attempt, written exactly the way `BookingService::create_booking`
    /// writes it: lock the spot, *then* read availability, insert only if free.
    ///
    /// `lock` is the whole variable under test.
    async fn try_reserve(pool: &shared::db::Db, spot_id: Uuid, lock: bool) -> bool {
        // A connection of its own: the two racers must not share one, or they would
        // serialise on the connection rather than on the row lock under test.
        let mut conn = shared::db::conn(pool).await.unwrap();

        conn.transaction::<bool, shared::error::myerror::MyError, _>(|conn| {
            async move {
                if lock {
                    SpotMirrorRepository::find_for_update(conn, spot_id)
                        .await
                        .unwrap();
                } else {
                    SpotMirrorRepository::find_by_id(conn, spot_id)
                        .await
                        .unwrap();
                }

                // Both racers pause here, so each has definitely reached this point
                // before either commits. Without the lock that means both read an empty
                // set; with it, the second is still blocked above and has not read
                // anything yet.
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;

                let taken = BookingRepository::taken_for_spot(conn, &spot_id, Utc::now())
                    .await
                    .unwrap();
                if !taken.is_empty() {
                    // Returning rather than rolling back: nothing has been written, so
                    // the two are the same except that this also releases the lock.
                    return Ok(false);
                }

                BookingRepository::upsert(
                    conn,
                    a_booking(Uuid::now_v7(), spot_id, Utc::now() + TimeDelta::hours(2)),
                )
                .await
                .unwrap();
                Ok(true)
            }
            .scope_boxed()
        })
        .await
        .unwrap()
    }

    /// Creates the spot mirror row, then races two reserves against it.
    ///
    /// **The row has to exist**, and that is not test scaffolding — it is the
    /// precondition the lock depends on. `SELECT … FOR UPDATE` on a row that is not
    /// there locks *nothing*: there is no phantom to take, both transactions sail
    /// past, and the slot is sold twice. (Measured: this test double-booked until the
    /// mirror row was created.)
    ///
    /// `create_booking` is safe from that by construction — it `context_not_found`s on
    /// a missing spot on the very next line, so a reserve against an unmirrored spot
    /// is a 404 rather than an unlocked write. Worth knowing the dependency is there,
    /// because it is the same phantom that made a `host` table necessary in
    /// payment-service and then made an advisory lock the answer instead.
    async fn seed_mirror(pool: &shared::db::Db, spot_id: Uuid) {
        let mut conn = shared::db::conn(pool).await.unwrap();
        SpotMirrorRepository::merge(
            &mut conn,
            spot_id,
            SpotMirrorPatch {
                host_id: Some(Uuid::now_v7()),
                price_per_hour: Some(700),
                availability: Some(availability()),
                timezone: Some("Europe/Brussels".to_string()),
                active: Some(true),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    }

    async fn race(pool: &shared::db::Db, spot_id: Uuid, lock: bool) -> usize {
        seed_mirror(pool, spot_id).await;
        let (a, b) = tokio::join!(
            try_reserve(pool, spot_id, lock),
            try_reserve(pool, spot_id, lock)
        );
        [a, b].iter().filter(|won| **won).count()
    }

    /// **The test the whole Read Committed design rests on.**
    ///
    /// Two renters race one slot. They insert two *different* booking rows, so nothing
    /// collides on its own — under TiKV a counter on the spot was bumped to manufacture
    /// a conflict, and under Read Committed that bump would not conflict at all.
    ///
    /// What replaces it is the row lock plus a property of Read Committed: after the
    /// second transaction unblocks, its **next statement takes a new snapshot** and so
    /// sees the winner's booking. Exactly one insert survives.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn two_racing_reserves_cannot_double_book() {
        let pool = db().await;
        let spot_id = Uuid::now_v7();

        assert_eq!(
            race(&pool, spot_id, true).await,
            1,
            "exactly one of two racing reserves may take the slot"
        );

        diesel::delete(booking::table.filter(booking::spot_id.eq(spot_id)))
            .execute(&mut *conn(&pool).await)
            .await
            .unwrap();
    }

    /// A second transaction starts *while* the first is still open and unreserved,
    /// then reads availability. Does it see the first's booking?
    ///
    /// This is the mechanism itself, isolated and made deterministic. Two racing
    /// `try_reserve`s cannot be a negative control, because without the lock nothing
    /// orders them: whether the second's read lands before or after the first's commit
    /// is a coin flip, so it double-books only *sometimes*. A control that passes
    /// intermittently is worse than none. Here the writer holds its transaction open
    /// for a known duration, so the answer is forced either way.
    async fn second_reader_sees_the_first(
        pool: &shared::db::Db,
        spot_id: Uuid,
        lock: bool,
    ) -> bool {
        let writer = {
            let pool = pool.clone();
            tokio::spawn(async move {
                let mut conn = shared::db::conn(&pool).await.unwrap();
                conn.transaction::<(), shared::error::myerror::MyError, _>(|conn| {
                    async move {
                        // The writer always locks — it is `create_booking`. What varies
                        // is whether the *reader* does.
                        SpotMirrorRepository::find_for_update(conn, spot_id)
                            .await
                            .unwrap();
                        BookingRepository::upsert(
                            conn,
                            a_booking(Uuid::now_v7(), spot_id, Utc::now() + TimeDelta::hours(2)),
                        )
                        .await
                        .unwrap();
                        // Held open, so the reader below is guaranteed to start inside it.
                        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                        Ok(())
                    }
                    .scope_boxed()
                })
                .await
                .unwrap();
            })
        };

        // Long enough that the writer is certainly inside its transaction, short
        // enough that it certainly has not committed.
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;

        let mut conn = shared::db::conn(pool).await.unwrap();
        let taken = conn
            .transaction::<_, shared::error::myerror::MyError, _>(|conn| {
                async move {
                    if lock {
                        // Blocks here until the writer commits. Read Committed then
                        // gives the NEXT statement a new snapshot — which is the entire
                        // mechanism.
                        SpotMirrorRepository::find_for_update(conn, spot_id)
                            .await
                            .unwrap();
                    } else {
                        SpotMirrorRepository::find_by_id(conn, spot_id)
                            .await
                            .unwrap();
                    }
                    BookingRepository::taken_for_spot(conn, &spot_id, Utc::now()).await
                }
                .scope_boxed()
            })
            .await
            .unwrap();
        writer.await.unwrap();

        !taken.is_empty()
    }

    /// The negative control, and it is not optional.
    ///
    /// A concurrency test that passes for the wrong reason is worse than no test: if
    /// the unlocked reader *also* saw the booking, the lock would not be what is
    /// saving us and the real mechanism would be unidentified.
    ///
    /// The `false` case is also the exact failure a cluster below v2025.2 produces
    /// **with** the lock, because Read Committed silently degrades to Snapshot there
    /// and the second transaction re-reads its original snapshot after unblocking.
    /// Same stale read, no error either way — which is why the image tag in
    /// docker-compose-dev.yml is pinned as a correctness constraint rather than a
    /// preference.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn the_lock_is_what_makes_the_second_read_fresh() {
        let pool = db().await;

        let locked = Uuid::now_v7();
        seed_mirror(&pool, locked).await;
        assert!(
            second_reader_sees_the_first(&pool, locked, true).await,
            "with FOR UPDATE the second reader must block and then see the booking"
        );

        let unlocked = Uuid::now_v7();
        seed_mirror(&pool, unlocked).await;
        assert!(
            !second_reader_sees_the_first(&pool, unlocked, false).await,
            "without FOR UPDATE the second reader must see a stale, empty set — if it \
             does not, the lock is not what prevents double-booking and the real \
             mechanism is unknown"
        );

        diesel::delete(booking::table.filter(booking::spot_id.eq_any([locked, unlocked])))
            .execute(&mut *conn(&pool).await)
            .await
            .unwrap();
        diesel::delete(spot::table.filter(spot::id.eq_any([locked, unlocked])))
            .execute(&mut *conn(&pool).await)
            .await
            .unwrap();
    }
}
