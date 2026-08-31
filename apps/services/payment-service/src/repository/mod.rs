//! One repository per table, each holding the SQL for the statements this service
//! actually issues. There is no shared `Repository` trait behind them and nothing is
//! generated: what a method does is the string in front of you.
//!
//! Three tables: this service's own `payment` and `payout`, plus a mirror of the
//! bookings they pay for.
//!
//! Every statement is a literal — no `format!`, no consts spliced in from the domain
//! models, nothing to follow to a second file. Reads are plain `SELECT *`; writes name
//! their columns, because sqlx has no whole-struct write to match `CONTENT $row`.
//!
//! The repositories are stateless. They were generic over a `Querier` so one type
//! could serve a service and a projector; sqlx's `PgExecutor` covers `&PgPool` and
//! `&mut PgConnection` alike, so each method just takes one.

pub mod booking_mirror_repository;
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
    use shared::domain_models::booking::status as booking_status;
    use shared::domain_models::payment::{
        BookingMirror, BookingMirrorPatch, Payment, PaymentPatch, Payout, status,
    };
    use sqlx::PgPool;
    use uuid::Uuid;

    use super::booking_mirror_repository::BookingMirrorRepository;
    use super::payment_repository::PaymentRepository;
    use super::payout_repository::PayoutRepository;

    /// Connects and migrates, so a running container is the only prerequisite.
    async fn db() -> PgPool {
        let pool = shared::db::connect("postgres://yugabyte@127.0.0.1:5433/payment")
            .await
            .expect("dev yugabyte on :5433, database `payment` — see this module's docs");
        shared::db::migrate(&pool, &sqlx::migrate!("../../../migrations/payment"))
            .await
            .expect("migrations apply");
        pool
    }

    fn a_booking(id: Uuid, owner_id: Uuid, ends_at: chrono::DateTime<Utc>) -> BookingMirror {
        BookingMirror {
            id,
            spot_id: Uuid::now_v7(),
            owner_id,
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

    fn a_payment(id: Uuid, booking_id: Uuid, owner_id: Uuid, amount_cents: i64) -> Payment {
        Payment {
            id,
            version: 1,
            booking_id,
            owner_id,
            renter_id: Uuid::now_v7(),
            amount_cents,
            session_id: format!("cs_test_{id}"),
            intent_id: None,
            status: status::CREATED.to_string(),
            refund_id: None,
            failure_reason: None,
            created_at: Utc::now(),
        }
    }

    #[tokio::test]
    #[ignore]
    async fn a_payment_round_trips_and_patches_leave_absent_columns_alone() {
        let db = db().await;

        let (id, booking_id) = (Uuid::now_v7(), Uuid::now_v7());
        let row = a_payment(id, booking_id, Uuid::now_v7(), 1234);
        let session_id = row.session_id.clone();

        PaymentRepository::upsert(&db, row).await.unwrap();

        // Both UNIQUE columns address the same row.
        let got = PaymentRepository::find_by_booking_id(&db, booking_id)
            .await
            .unwrap()
            .expect("upserted row");
        assert_eq!(got.id, id);
        assert_eq!(got.version, 1);
        assert_eq!(got.amount_cents, 1234);
        assert_eq!(got.intent_id, None);
        let by_session = PaymentRepository::find_by_session_id(&db, session_id)
            .await
            .unwrap();
        assert_eq!(by_session.expect("same row").id, id);

        // `succeeded` sets status and intent together — the pairing the refund path
        // leans on, since it matches 'succeeded' and then needs the intent.
        PaymentRepository::transition(
            &db,
            id,
            &status::UNPAID,
            PaymentPatch::succeeded("pi_123".to_string()),
        )
        .await
        .unwrap();
        let got = PaymentRepository::find_by_booking_id(&db, booking_id)
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
            &db,
            id,
            &status::UNPAID,
            PaymentPatch::failed("card_declined".to_string()),
        )
        .await
        .unwrap();
        let got = PaymentRepository::find_by_booking_id(&db, booking_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.status, status::SUCCEEDED, "the guard must have held");
        assert_eq!(got.failure_reason, None);

        sqlx::query("DELETE FROM payment WHERE id = $1")
            .bind(id)
            .execute(&db)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[ignore]
    async fn a_booking_round_trips_its_map_and_clears_its_hold_on_transition() {
        let db = db().await;

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
        )]);
        let booked = row.booked.clone();

        BookingMirrorRepository::upsert(&db, row).await.unwrap();

        let got = BookingMirrorRepository::find_by_id(&db, id)
            .await
            .unwrap()
            .expect("upserted row");
        assert_eq!(got.booked, booked, "the jsonb column must round-trip");
        assert!(got.hold_until.is_some());

        BookingMirrorRepository::transition(
            &db,
            id,
            &[booking_status::RESERVED],
            BookingMirrorPatch::status(booking_status::CONFIRMED),
        )
        .await
        .unwrap();

        let got = BookingMirrorRepository::find_by_id(&db, id).await.unwrap().unwrap();
        assert_eq!(got.status, booking_status::CONFIRMED);
        // The unconditional assignment, not the patch: `COALESCE` can leave a value
        // alone but never clear it, and every transition out of `reserved` ends the
        // hold.
        assert_eq!(got.hold_until, None, "every transition ends the hold");
        assert_eq!(got.booked, booked, "a transition must not disturb the rest");

        sqlx::query("DELETE FROM booking WHERE id = $1")
            .bind(id)
            .execute(&db)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[ignore]
    async fn payouts_sum_per_owner() {
        let db = db().await;

        let owner_id = Uuid::now_v7();
        let (a, b) = (Uuid::now_v7(), Uuid::now_v7());
        for (id, amount_cents) in [(a, 700), (b, 300)] {
            PayoutRepository::upsert(
                &db,
                Payout {
                    id,
                    version: 1,
                    owner_id,
                    amount_cents,
                    created_at: Utc::now(),
                },
            )
            .await
            .unwrap();
        }

        assert_eq!(PayoutRepository::total_for(&db, &owner_id).await.unwrap(), 1000);
        // A host with nothing withdrawn is 0, not an error and not NULL — SUM over no
        // rows is NULL, which is what the COALESCE in that statement is for.
        assert_eq!(
            PayoutRepository::total_for(&db, &Uuid::now_v7()).await.unwrap(),
            0
        );

        sqlx::query("DELETE FROM payout WHERE id = ANY($1)")
            .bind(vec![a, b])
            .execute(&db)
            .await
            .unwrap();
    }

    /// Seeds one settled, paid booking worth `cents` for a fresh host.
    async fn seed_earnings(db: &PgPool, owner_id: Uuid, cents: i64) {
        let booking_id = Uuid::now_v7();
        // Ended in the past, so it is past any settlement window.
        BookingMirrorRepository::upsert(
            db,
            a_booking(booking_id, owner_id, Utc::now() - TimeDelta::days(3)),
        )
        .await
        .unwrap();

        let payment_id = Uuid::now_v7();
        PaymentRepository::upsert(db, a_payment(payment_id, booking_id, owner_id, cents))
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
    /// `PaymentService::request_payout` writes it: lock the owner, *then* compute the
    /// balance, insert only if there is something to take.
    ///
    /// `lock` and `read_before_lock` are the two variables under test — they are the
    /// two halves of the fix, and the point is that neither works alone.
    async fn try_withdraw(
        db: &PgPool,
        owner_id: Uuid,
        lock: bool,
        read_before_lock: bool,
    ) -> Option<i64> {
        let mut tx = db.begin().await.unwrap();

        let early = if read_before_lock {
            Some(balance(&mut tx, &owner_id).await)
        } else {
            None
        };

        if lock {
            PayoutRepository::lock_owner(&mut *tx, &owner_id).await.unwrap();
        }

        // Both racers are certainly past the lock decision before either commits.
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;

        let amount = match early {
            Some(stale) => stale,
            None => balance(&mut tx, &owner_id).await,
        };

        if amount <= 0 {
            tx.rollback().await.unwrap();
            return None;
        }

        PayoutRepository::upsert(
            &mut *tx,
            Payout {
                id: Uuid::now_v7(),
                version: 1,
                owner_id,
                amount_cents: amount,
                created_at: Utc::now(),
            },
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        Some(amount)
    }

    /// Two unlocked withdrawals, forced to both read before either commits.
    ///
    /// Returns the total paid out. This is the interleaving the advisory lock makes
    /// impossible, staged deterministically instead of hoped for.
    async fn unlocked_pair(db: &PgPool, owner_id: Uuid) -> i64 {
        let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
        let mut tasks = Vec::new();

        for _ in 0..2 {
            let db = db.clone();
            let barrier = barrier.clone();
            tasks.push(tokio::spawn(async move {
                let mut tx = db.begin().await.unwrap();
                let amount = balance(&mut tx, &owner_id).await;
                // Neither may commit until both have read.
                barrier.wait().await;
                if amount <= 0 {
                    tx.rollback().await.unwrap();
                    return 0;
                }
                PayoutRepository::upsert(
                    &mut *tx,
                    Payout {
                        id: Uuid::now_v7(),
                        version: 1,
                        owner_id,
                        amount_cents: amount,
                        created_at: Utc::now(),
                    },
                )
                .await
                .unwrap();
                tx.commit().await.unwrap();
                amount
            }));
        }

        let mut total = 0;
        for t in tasks {
            total += t.await.unwrap();
        }
        total
    }

    async fn balance(conn: &mut sqlx::PgConnection, owner_id: &Uuid) -> i64 {
        let earned = PaymentRepository::earned(&mut *conn, owner_id, Utc::now())
            .await
            .unwrap();
        let paid = PayoutRepository::total_for(&mut *conn, owner_id).await.unwrap();
        earned - paid
    }

    /// **The test the money path rests on.**
    ///
    /// A balance is derived, never stored, so two double-clicked withdrawals insert two
    /// *different* payout rows and nothing collides on its own. Under TiKV a `host`
    /// table existed purely to be bumped into a write conflict; under Read Committed
    /// that bump would not conflict, so the table is gone and an advisory lock on the
    /// owner id takes its place.
    #[tokio::test]
    #[ignore]
    async fn two_concurrent_withdrawals_pay_out_once() {
        let db = db().await;
        let owner_id = Uuid::now_v7();
        seed_earnings(&db, owner_id, 5000).await;

        let (a, b) = tokio::join!(
            try_withdraw(&db, owner_id, true, false),
            try_withdraw(&db, owner_id, true, false)
        );

        let paid: Vec<i64> = [a, b].into_iter().flatten().collect();
        assert_eq!(paid, vec![5000], "exactly one withdrawal may take the money");
        assert_eq!(
            PayoutRepository::total_for(&db, &owner_id).await.unwrap(),
            5000,
            "the host must not be able to withdraw more than they earned"
        );

        cleanup(&db, owner_id).await;
    }

    /// The negative controls, and they are not optional — one per half of the fix.
    ///
    /// If either of these passed, the corresponding half would not be what is saving
    /// us and the real mechanism would be unidentified.
    #[tokio::test]
    #[ignore]
    async fn both_halves_of_the_payout_fix_are_load_bearing() {
        let db = db().await;

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
        seed_earnings(&db, no_lock, 5000).await;
        assert_eq!(
            unlocked_pair(&db, no_lock).await,
            10000,
            "without the advisory lock the money must go out twice — if it does not, \
             the lock is not what prevents it and the real mechanism is unknown"
        );

        // Lock held, but the balance read hoisted above it — which is exactly where
        // `available_for` used to sit, before the transaction opened. The lock serialises the two
        // and changes nothing, because the loser's figure was already stale.
        let stale = Uuid::now_v7();
        seed_earnings(&db, stale, 5000).await;
        let (a, b) = tokio::join!(
            try_withdraw(&db, stale, true, true),
            try_withdraw(&db, stale, true, true)
        );
        assert_eq!(
            [a, b].into_iter().flatten().sum::<i64>(),
            10000,
            "a balance read taken before the lock is stale however long the lock is \
             held — this is why the read had to move inside the transaction"
        );

        cleanup(&db, no_lock).await;
        cleanup(&db, stale).await;
    }

    async fn cleanup(db: &PgPool, owner_id: Uuid) {
        for sql in [
            "DELETE FROM payout WHERE owner_id = $1",
            "DELETE FROM payment WHERE owner_id = $1",
            "DELETE FROM booking WHERE owner_id = $1",
        ] {
            sqlx::query(sql).bind(owner_id).execute(db).await.unwrap();
        }
    }
}
