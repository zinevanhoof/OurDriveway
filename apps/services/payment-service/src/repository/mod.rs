//! One repository per table, each holding the SQL for the statements this service
//! actually issues. There is no shared `Repository` trait behind them and nothing is
//! generated: what a method does is the string in front of you.
//!
//! Five tables: this service's own `payment`, `payout` and `connect_account`, plus
//! mirrors of the bookings they pay for and of the hosts they pay.
//!
//! Reads are `Selectable`; writes bind the struct whole through `Insertable` and
//! `AsChangeset`, which is what `CONTENT $row` did and what sqlx could not. Three
//! statements remain `sql_query`, each for a reason spelled out where it appears:
//! `COALESCE(SUM(…), 0)::bigint` twice — Postgres widens `SUM(bigint)` to numeric,
//! which does not decode into an `i64` — and `connect_account`'s CTE, which diesel has
//! no builder for.
//!
//! **`sql_query` is the exception and needs a reason at the call site.** Four statements
//! here were hand-written strings with no reason at all, over tables that are in
//! `shared::schema::payment` and therefore have a compile-checked spelling: all three of
//! `host_mirror_repository`'s, and `connect_account_repository::find`.
//!
//! The repositories are stateless. They were generic over a `Querier` so one type
//! could serve a service and a projector; every method now takes
//! `&mut AsyncPgConnection`, which is what a pooled connection and an open transaction
//! both are.

pub mod booking_mirror_repository;
pub mod connect_account_repository;
pub mod host_mirror_repository;
pub mod payment_repository;
pub mod payout_repository;

/// Round-trips each table through a real YugabyteDB, and proves a host cannot
/// withdraw twice.
///
/// `#[ignore]`d — needs the dev cluster on :5433, and CI runs
/// `cargo test --workspace` with no database:
///
/// ```sh
/// docker compose -f docker/docker-compose-dev.yml up -d yugabyte
/// cargo test --workspace -- --ignored
/// ```
///
/// `two_concurrent_withdrawals_pay_out_once` is the one that has to exist. This is the
/// money path, and what protects it is not visible in any single statement: it is an
/// advisory lock plus the *order* of two calls in `request_payout`. Neither the
/// compiler nor a schema can check that, so a database can.
#[cfg(test)]
mod live_tests {
    use chrono::{TimeDelta, Utc};
    use diesel::prelude::*;
    use diesel_async::scoped_futures::ScopedFutureExt;
    use diesel_async::{AsyncConnection, RunQueryDsl};
    use shared::domain_models::booking::status as booking_status;
    use shared::domain_models::payment::payout::status as payout_status;
    use shared::domain_models::payment::{
        BookingMirror, BookingMirrorPatch, Payment, PaymentPatch, Payout, PayoutPatch, status,
    };
    use shared::schema::payment::{booking, host, payment, payout};
    use uuid::Uuid;

    use super::booking_mirror_repository::BookingMirrorRepository;
    use super::host_mirror_repository::HostMirrorRepository;
    use super::payment_repository::PaymentRepository;
    use super::payout_repository::PayoutRepository;

    /// Connects and migrates, so a running container is the only prerequisite.
    async fn db() -> shared::db::Db {
        // SAFETY: tests in one binary share an environment and every caller sets the
        // same value.
        unsafe {
            std::env::set_var(
                "PAYMENT_DATABASE_URL",
                "postgres://yugabyte@127.0.0.1:5433/payment",
            )
        };
        // Through the migrator rather than a second copy of the wiring: that crate is
        // the only thing that migrates in dev and production, so a test cannot drift
        // from what actually gets applied. It creates the database if missing.
        //
        // `ensure` and not `run_one`: test threads run in parallel and would otherwise
        // all try to apply a new migration at once, which YugabyteDB refuses rather than
        // serialises. See the note on that function.
        migrator::ensure("payment").await.expect("migrations apply");

        shared::db::connect("postgres://yugabyte@127.0.0.1:5433/payment")
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

    fn a_booking(id: Uuid, host_id: Uuid, ends_at: chrono::DateTime<Utc>) -> BookingMirror {
        BookingMirror {
            id,
            spot_id: Uuid::now_v7(),
            host_id,
            renter_id: Uuid::now_v7(),
            amount_cents: 1000,
            booked: Default::default(),
            status: booking_status::CONFIRMED.to_string(),
            hold_until: None,
            ends_at,
            cancel_reason: None,
            release_reason: None,
        }
    }

    /// A freshly requested payout — the state `request_payout` commits, before the
    /// worker has been anywhere near Stripe.
    fn a_payout(id: Uuid, host_id: Uuid, amount_cents: i64) -> Payout {
        Payout {
            id,
            version: 1,
            host_id,
            amount_cents,
            status: payout_status::REQUESTED.to_string(),
            transfer_id: None,
            failure_reason: None,
            created_at: Utc::now(),
        }
    }

    fn a_payment(id: Uuid, booking_id: Uuid, host_id: Uuid, amount_cents: i64) -> Payment {
        Payment {
            id,
            version: 1,
            booking_id,
            host_id,
            renter_id: Uuid::now_v7(),
            amount_cents,
            session_id: format!("cs_test_{id}"),
            intent_id: None,
            status: status::CREATED.to_string(),
            refund_id: None,
            refunded_at: None,
            failure_reason: None,
            created_at: Utc::now(),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn a_payment_round_trips_and_patches_leave_absent_columns_alone() {
        let pool = db().await;
        let db = &mut *conn(&pool).await;

        let (id, booking_id) = (Uuid::now_v7(), Uuid::now_v7());
        let row = a_payment(id, booking_id, Uuid::now_v7(), 1234);
        let session_id = row.session_id.clone();

        PaymentRepository::upsert(db, row).await.unwrap();

        // Both UNIQUE columns address the same row.
        let got = PaymentRepository::find_by_booking_id(db, booking_id)
            .await
            .unwrap()
            .expect("upserted row");
        assert_eq!(got.id, id);
        assert_eq!(got.version, 1);
        assert_eq!(got.amount_cents, 1234);
        assert_eq!(got.intent_id, None);
        let by_session = PaymentRepository::find_by_session_id(db, session_id)
            .await
            .unwrap();
        assert_eq!(by_session.expect("same row").id, id);

        // `succeeded` sets status and intent together — the pairing the refund path
        // leans on, since it matches 'succeeded' and then needs the intent.
        PaymentRepository::transition(
            db,
            id,
            &status::UNPAID,
            PaymentPatch::succeeded("pi_123".to_string()),
        )
        .await
        .unwrap();
        let got = PaymentRepository::find_by_booking_id(db, booking_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.status, status::SUCCEEDED);
        assert_eq!(got.intent_id.as_deref(), Some("pi_123"));
        assert_eq!(got.amount_cents, 1234, "absent columns survive a patch");
        assert_eq!(got.failure_reason, None);

        // Redelivery: no longer UNPAID, so the guard refuses. Money states only move
        // forwards, and this is what makes that true rather than hoped.
        PaymentRepository::transition(
            db,
            id,
            &status::UNPAID,
            PaymentPatch::failed("card_declined".to_string()),
        )
        .await
        .unwrap();
        let got = PaymentRepository::find_by_booking_id(db, booking_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.status, status::SUCCEEDED, "the guard must have held");
        assert_eq!(got.failure_reason, None);

        diesel::delete(payment::table.find(id))
            .execute(&mut *conn(&pool).await)
            .await
            .unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn a_booking_round_trips_its_map_and_clears_its_hold_on_transition() {
        let pool = db().await;
        let db = &mut *conn(&pool).await;

        let id = Uuid::now_v7();
        let mut row = a_booking(id, Uuid::now_v7(), Utc::now() + TimeDelta::hours(2));
        row.status = booking_status::RESERVED.to_string();
        row.hold_until = Some(Utc::now() + TimeDelta::minutes(10));
        row.booked = std::collections::HashMap::from([(
            "2026-08-19".to_string(),
            vec![shared::general_models::spot::TimeSlot {
                start: "09:00".to_string(),
                end: "11:00".to_string(),
            }],
        )])
        .into();
        let booked = row.booked.clone();

        BookingMirrorRepository::upsert(db, row).await.unwrap();

        let got = BookingMirrorRepository::find_by_id(db, id)
            .await
            .unwrap()
            .expect("upserted row");
        assert_eq!(got.booked, booked, "the jsonb column must round-trip");
        assert!(got.hold_until.is_some());

        BookingMirrorRepository::transition(
            db,
            id,
            &[booking_status::RESERVED],
            BookingMirrorPatch::status(booking_status::CONFIRMED),
        )
        .await
        .unwrap();

        let got = BookingMirrorRepository::find_by_id(db, id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.status, booking_status::CONFIRMED);
        // The unconditional assignment, not the patch: `COALESCE` can leave a value
        // alone but never clear it, and every transition out of `reserved` ends the
        // hold.
        assert_eq!(got.hold_until, None, "every transition ends the hold");
        assert_eq!(got.booked, booked, "a transition must not disturb the rest");

        diesel::delete(booking::table.find(id))
            .execute(&mut *conn(&pool).await)
            .await
            .unwrap();
    }

    /// **The regression test for a silent rollback.**
    ///
    /// `UserProjector` writes the `host` mirror and then calls `db::set_version` for
    /// the `user` aggregate — whose table, `app_user`, this service does not have. That
    /// used to be handled by letting the statement fail and swallowing 42P01, which
    /// aborts the transaction the projector opened: the swallow answered `Ok`, the
    /// commit became a silent rollback, the message was acked, and the mirror stayed
    /// empty through eighteen events with nothing in any log.
    ///
    /// So this applies a real event through the real projector, **commits**, and reads
    /// the row back on a fresh connection. The commit is the whole point — asserting
    /// inside the transaction would have passed the entire time the bug existed.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn a_projected_user_survives_the_commit_though_this_service_has_no_user_table() {
        use bus::Projector;
        use shared::events::user::{UserRegistered, UserUpdated};

        let pool = db().await;
        let db = &mut *conn(&pool).await;
        let user_id = Uuid::now_v7();
        let email = format!("host-{user_id}@example.test");

        #[derive(diesel::QueryableByName)]
        struct Regclass {
            #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
            to_regclass: Option<String>,
        }

        assert!(
            diesel::sql_query("SELECT to_regclass('app_user')::text AS to_regclass")
                .load::<Regclass>(&mut *conn(&pool).await)
                .await
                .unwrap()
                .into_iter()
                .next()
                .and_then(|r| r.to_regclass)
                .is_none(),
            "the premise: payment-service has no app_user table, which is what made \
             set_version poison the projector's transaction"
        );

        // Each event in its own transaction, exactly as `bus::Tx` runs it — which is
        // the whole point of this test: `set_version` must not poison a transaction that
        // then commits nothing.
        let apply = async |event, version| {
            let mut conn = shared::db::conn(&pool).await.unwrap();
            conn.transaction::<(), shared::error::myerror::MyError, _>(|conn| {
                async move {
                    crate::projector::UserProjector
                        .apply(conn, event, Utc::now(), version)
                        .await
                        .unwrap();
                    Ok(())
                }
                .scope_boxed()
            })
            .await
            .unwrap();
        };

        apply(
            shared::events::user::UserEvent::Registered(UserRegistered {
                user_id,
                first_name: "Ada".to_string(),
                last_name: "Lovelace".to_string(),
                email: email.clone(),
                password_hash: "$argon2id$vTEST".to_string(),
            }),
            1,
        )
        .await;

        let host = HostMirrorRepository::find(db, &user_id)
            .await
            .unwrap()
            .expect("Registered creates the mirror row");
        assert_eq!(host.email, email);
        assert_eq!(host.country, None, "signup never carries a country");

        // The country arrives on a later profile edit and nothing else changes. This is
        // the path a host actually takes before onboarding.
        apply(
            shared::events::user::UserEvent::Updated(UserUpdated {
                user_id,
                first_name: None,
                last_name: None,
                email: None,
                profile_picture: None,
                license_plates: None,
                country: Some("BE".to_string()),
            }),
            2,
        )
        .await;

        let host = HostMirrorRepository::find(db, &user_id)
            .await
            .unwrap()
            .expect("the row is still there");
        assert_eq!(host.country.as_deref(), Some("BE"));
        assert_eq!(
            host.email, email,
            "an absent field is unchanged, not cleared"
        );

        // **A redelivered `Registered` must not undo the country.** This is the reason
        // `HostMirrorRepository::upsert` lists its two conflict columns rather than
        // writing the row whole: a `Registered` carries no country, so a whole-row
        // `.set()` would clear one the host has since set — and Stripe fixes the country
        // permanently at account creation, so clearing it costs them their payouts.
        //
        // A replay is ordinary, not exotic: NATS redelivers and the projector reapplies.
        apply(
            shared::events::user::UserEvent::Registered(UserRegistered {
                user_id,
                first_name: "Ada".to_string(),
                last_name: "Lovelace".to_string(),
                email: email.clone(),
                password_hash: "$argon2id$vTEST".to_string(),
            }),
            1,
        )
        .await;

        assert_eq!(
            HostMirrorRepository::find(db, &user_id)
                .await
                .unwrap()
                .expect("the row is still there")
                .country
                .as_deref(),
            Some("BE"),
            "a replayed Registered must not clear a country it does not carry"
        );

        diesel::delete(host::table.find(user_id))
            .execute(&mut *conn(&pool).await)
            .await
            .unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn payouts_sum_per_host() {
        let pool = db().await;
        let db = &mut *conn(&pool).await;

        let host_id = Uuid::now_v7();
        let (a, b) = (Uuid::now_v7(), Uuid::now_v7());
        for (id, amount_cents) in [(a, 700), (b, 300)] {
            PayoutRepository::upsert(db, a_payout(id, host_id, amount_cents))
                .await
                .unwrap();
        }

        assert_eq!(
            PayoutRepository::total_for(db, &host_id).await.unwrap(),
            1000,
            "a requested payout counts — the money is already spoken for"
        );

        // The refund mechanism, and there is no other one: a failed transfer stops
        // counting, and the balance goes back up because it is derived on every read.
        PayoutRepository::transition(
            db,
            b,
            &[payout_status::REQUESTED],
            PayoutPatch::failed("balance_insufficient".to_string()),
        )
        .await
        .unwrap();
        assert_eq!(
            PayoutRepository::total_for(db, &host_id).await.unwrap(),
            700,
            "a failed payout must hand the money back"
        );

        // Paid still counts, and the transfer id is what a reconciliation would need.
        PayoutRepository::transition(
            db,
            a,
            &[payout_status::REQUESTED],
            PayoutPatch::paid("tr_test".to_string()),
        )
        .await
        .unwrap();
        let paid = PayoutRepository::find_by_id(db, a)
            .await
            .unwrap()
            .expect("upserted row");
        assert_eq!(paid.status, payout_status::PAID);
        assert_eq!(paid.transfer_id.as_deref(), Some("tr_test"));
        assert_eq!(paid.amount_cents, 700, "absent columns survive a patch");
        assert_eq!(
            PayoutRepository::total_for(db, &host_id).await.unwrap(),
            700
        );

        // Redelivery: no longer `requested`, so the guard refuses and a second transfer
        // cannot overwrite the first's id.
        PayoutRepository::transition(
            db,
            a,
            &[payout_status::REQUESTED],
            PayoutPatch::paid("tr_second".to_string()),
        )
        .await
        .unwrap();
        assert_eq!(
            PayoutRepository::find_by_id(db, a)
                .await
                .unwrap()
                .unwrap()
                .transfer_id
                .as_deref(),
            Some("tr_test"),
            "the guard must have held"
        );
        // A host with nothing withdrawn is 0, not an error and not NULL — SUM over no
        // rows is NULL, which is what the COALESCE in that statement is for.
        assert_eq!(
            PayoutRepository::total_for(db, &Uuid::now_v7())
                .await
                .unwrap(),
            0
        );

        diesel::delete(payout::table.filter(payout::id.eq_any([a, b])))
            .execute(&mut *conn(&pool).await)
            .await
            .unwrap();
    }

    /// Seeds one settled, paid booking worth `cents` for a fresh host.
    async fn seed_earnings(pool: &shared::db::Db, host_id: Uuid, cents: i64) {
        let db = &mut *conn(pool).await;
        let booking_id = Uuid::now_v7();
        // Ended in the past, so it is past any settlement window.
        BookingMirrorRepository::upsert(
            db,
            a_booking(booking_id, host_id, Utc::now() - TimeDelta::days(3)),
        )
        .await
        .unwrap();

        let payment_id = Uuid::now_v7();
        PaymentRepository::upsert(db, a_payment(payment_id, booking_id, host_id, cents))
            .await
            .unwrap();
        PaymentRepository::transition(
            db,
            payment_id,
            &status::UNPAID,
            PaymentPatch::succeeded("pi_seed".to_string()),
        )
        .await
        .unwrap();
    }

    /// One withdrawal attempt, written exactly the way
    /// `PaymentService::request_payout` writes it: lock the host, *then* compute the
    /// balance, insert only if there is something to take.
    ///
    /// `lock` and `read_before_lock` are the two variables under test — they are the
    /// two halves of the fix, and the point is that neither works alone.
    async fn try_withdraw(
        pool: &shared::db::Db,
        host_id: Uuid,
        lock: bool,
        read_before_lock: bool,
    ) -> Option<i64> {
        // A connection of its own, or the two racers would serialise on the connection
        // rather than on the advisory lock under test.
        let mut conn = shared::db::conn(pool).await.unwrap();

        conn.transaction::<Option<i64>, shared::error::myerror::MyError, _>(|conn| {
            async move {
                let early = if read_before_lock {
                    Some(balance(conn, &host_id).await)
                } else {
                    None
                };

                if lock {
                    PayoutRepository::lock_host(conn, &host_id).await.unwrap();
                }

                // Both racers are certainly past the lock decision before either commits.
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;

                let amount = match early {
                    Some(stale) => stale,
                    None => balance(conn, &host_id).await,
                };

                if amount <= 0 {
                    // Nothing written, so returning and rolling back are the same — and
                    // returning also releases the advisory lock, which is xact-scoped.
                    return Ok(None);
                }

                PayoutRepository::upsert(conn, a_payout(Uuid::now_v7(), host_id, amount))
                    .await
                    .unwrap();
                Ok(Some(amount))
            }
            .scope_boxed()
        })
        .await
        .unwrap()
    }

    /// Two unlocked withdrawals, forced to both read before either commits.
    ///
    /// Returns the total paid out. This is the interleaving the advisory lock makes
    /// impossible, staged deterministically instead of hoped for.
    async fn unlocked_pair(db: &shared::db::Db, host_id: Uuid) -> i64 {
        let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
        let mut tasks = Vec::new();

        for _ in 0..2 {
            let db = db.clone();
            let barrier = barrier.clone();
            tasks.push(tokio::spawn(async move {
                let mut conn = shared::db::conn(&db).await.unwrap();
                conn.transaction::<i64, shared::error::myerror::MyError, _>(|conn| {
                    async move {
                        let amount = balance(conn, &host_id).await;
                        // Neither may commit until both have read.
                        barrier.wait().await;
                        if amount <= 0 {
                            return Ok(0);
                        }
                        PayoutRepository::upsert(conn, a_payout(Uuid::now_v7(), host_id, amount))
                            .await
                            .unwrap();
                        Ok(amount)
                    }
                    .scope_boxed()
                })
                .await
                .unwrap()
            }));
        }

        let mut total = 0;
        for t in tasks {
            total += t.await.unwrap();
        }
        total
    }

    async fn balance(conn: &mut diesel_async::AsyncPgConnection, host_id: &Uuid) -> i64 {
        let earned = PaymentRepository::earned(&mut *conn, host_id, Utc::now())
            .await
            .unwrap();
        let paid = PayoutRepository::total_for(&mut *conn, host_id)
            .await
            .unwrap();
        earned - paid
    }

    /// **The test the money path rests on.**
    ///
    /// A balance is derived, never stored, so two double-clicked withdrawals insert two
    /// *different* payout rows and nothing collides on its own. Under TiKV a `host`
    /// table existed purely to be bumped into a write conflict; under Read Committed
    /// that bump would not conflict, so the table is gone and an advisory lock on the
    /// host id takes its place.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn two_concurrent_withdrawals_pay_out_once() {
        let pool = db().await;
        let db = &mut *conn(&pool).await;
        let host_id = Uuid::now_v7();
        seed_earnings(&pool, host_id, 5000).await;

        let (a, b) = tokio::join!(
            try_withdraw(&pool, host_id, true, false),
            try_withdraw(&pool, host_id, true, false)
        );

        let paid: Vec<i64> = [a, b].into_iter().flatten().collect();
        assert_eq!(
            paid,
            vec![5000],
            "exactly one withdrawal may take the money"
        );
        assert_eq!(
            PayoutRepository::total_for(db, &host_id).await.unwrap(),
            5000,
            "the host must not be able to withdraw more than they earned"
        );

        cleanup(&pool, host_id).await;
    }

    /// The negative controls, and they are not optional — one per half of the fix.
    ///
    /// If either of these passed, the corresponding half would not be what is saving
    /// us and the real mechanism would be unidentified.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn both_halves_of_the_payout_fix_are_load_bearing() {
        let pool = db().await;

        // No lock: both compute 5000 and both insert. 10000 out of 5000 earned.
        //
        // Barriered rather than raced on a sleep, and the difference matters. Two
        // unlocked withdrawals racing on timing double-pay only *sometimes* — whether
        // the second's read lands before or after the first's commit is unsynchronised
        // — so the assertion would pass intermittently, which is worse than no test.
        // The barrier forces both to read before either commits, which is precisely
        // the interleaving the lock exists to make impossible. It cannot be used on
        // the locked path: the second task would block on the lock and never reach the
        // barrier the first is waiting at.
        let no_lock = Uuid::now_v7();
        seed_earnings(&pool, no_lock, 5000).await;
        assert_eq!(
            unlocked_pair(&pool, no_lock).await,
            10000,
            "without the advisory lock the money must go out twice — if it does not, \
             the lock is not what prevents it and the real mechanism is unknown"
        );

        // Lock held, but the balance read hoisted above it — which is exactly where
        // `available_for` used to sit, before the transaction opened. The lock serialises the two
        // and changes nothing, because the loser's figure was already stale.
        let stale = Uuid::now_v7();
        seed_earnings(&pool, stale, 5000).await;
        let (a, b) = tokio::join!(
            try_withdraw(&pool, stale, true, true),
            try_withdraw(&pool, stale, true, true)
        );
        assert_eq!(
            [a, b].into_iter().flatten().sum::<i64>(),
            10000,
            "a balance read taken before the lock is stale however long the lock is \
             held — this is why the read had to move inside the transaction"
        );

        cleanup(&pool, no_lock).await;
        cleanup(&pool, stale).await;
    }

    async fn cleanup(pool: &shared::db::Db, host_id: Uuid) {
        let mut conn = shared::db::conn(pool).await.unwrap();
        // Three statements rather than a loop over three strings: the tables have
        // different types, so there is nothing for a loop to abstract over here.
        diesel::delete(payout::table.filter(payout::host_id.eq(host_id)))
            .execute(&mut *conn)
            .await
            .unwrap();
        diesel::delete(payment::table.filter(payment::host_id.eq(host_id)))
            .execute(&mut *conn)
            .await
            .unwrap();
        diesel::delete(booking::table.filter(booking::host_id.eq(host_id)))
            .execute(&mut *conn)
            .await
            .unwrap();
    }
}
