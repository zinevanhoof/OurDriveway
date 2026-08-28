//! One repository per table, each holding the SurrealQL for the statements this
//! service actually issues. There is no shared `Repository` trait behind them and
//! nothing is generated: what a method does is the string in front of you.
//!
//! Three tables: this service's own `payment` and `payout`, plus a mirror of the
//! bookings they pay for. Between them that is nine methods, which is every read and
//! write payment-service performs.
//!
//! Every statement is a literal — no `format!`, no consts spliced in from the domain
//! models, nothing to follow to a second file. Reads select `record::id(id) AS id`
//! plus whatever else comes back in a shape the struct cannot deserialize, then `*`.
//! Writes bind the struct whole with `CONTENT $row`. Both mean adding a column to a
//! model needs no edit here — only a *patchable* column does, in the one `SET` list
//! that names it.
//!
//! Each is generic over its querier so the same type serves both positions: the
//! service holds one over the pooled `Surreal<Client>`, a projector builds one over
//! the open `&Transaction` for a single event.

pub mod booking_mirror_repository;
pub mod payment_repository;
pub mod payout_repository;

/// Round-trips each table through a real SurrealDB.
///
/// `#[ignore]`d, because these need the shared SurrealDB up on :8000 with
/// `schemas/payment-schema.surql` imported into the `payment` database, and CI
/// runs `cargo test --workspace` with no database:
///
/// ```sh
/// docker compose -f docker/docker-compose-dev.yml up -d surrealdb schema-import
/// cargo test --workspace -- --ignored
/// ```
///
/// They earn their keep because the statements above are hand-written strings, and
/// two of the things they rely on cannot be checked any other way:
///
/// - `CONTENT $row` binds the struct whole against a **SCHEMAFULL** table, including
///   an `id` field the statement also names. SurrealDB requires the two to agree.
/// - `SELECT record::id(id) AS id, *` has to come back as something the struct can
///   deserialize — the record key unwrapped, and every other column raw.
///
/// A string assertion in `shared` can prove neither. Everything here writes rows
/// under fresh uuids and deletes them again.
#[cfg(test)]
mod live_tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use chrono::{TimeDelta, Utc};
    use shared::db::Querier;
    use shared::domain_models::booking::status as booking_status;
    use shared::domain_models::payment::{
        BookingMirror, BookingMirrorPatch, Payment, PaymentPatch, Payout, status,
    };
    use shared::general_models::spot::TimeSlot;
    use surrealdb::{Surreal, engine::remote::ws::Client};
    use uuid::Uuid;

    use super::booking_mirror_repository::BookingMirrorRepository;
    use super::payment_repository::PaymentRepository;
    use super::payout_repository::PayoutRepository;

    async fn db() -> Arc<Surreal<Client>> {
        Arc::new(
            shared::db::connect("127.0.0.1:8000", "root", "root", "payment")
                .await
                .expect("shared surrealdb on :8000, db `payment` — see this module's docs"),
        )
    }

    async fn drop_row(db: &Arc<Surreal<Client>>, table: &str, id: Uuid) {
        db.q(format!("DELETE type::record('{table}', $v)"))
            .bind(("v", id))
            .await
            .unwrap()
            .check()
            .unwrap();
    }

    #[tokio::test]
    #[ignore]
    async fn a_payment_round_trips_and_patches_leave_absent_columns_alone() {
        let db = db().await;
        let repo = PaymentRepository { q: db.clone() };

        let (id, booking_id) = (Uuid::now_v7(), Uuid::now_v7());
        let session_id = format!("cs_test_{id}");
        let row = Payment {
            id,
            version: 1,
            booking_id,
            owner_id: Uuid::now_v7(),
            renter_id: Uuid::now_v7(),
            amount_cents: 1234,
            session_id: session_id.clone(),
            intent_id: None,
            status: status::CREATED.to_string(),
            refund_id: None,
            failure_reason: None,
            created_at: Utc::now().into(),
        };

        repo.upsert(row.clone()).await.unwrap();

        let got = repo
            .find_by_booking_id(booking_id)
            .await
            .unwrap()
            .expect("upserted row");
        // The whole point of `record::id(id) AS id`: without it this is a RecordId
        // and does not deserialize.
        assert_eq!(got.id, id);
        assert_eq!(got.amount_cents, 1234);
        assert_eq!(got.session_id, session_id);
        assert_eq!(got.intent_id, None);
        assert_eq!(got.created_at, row.created_at);

        // The second UNIQUE column reaches the same row.
        let by_session = repo.find_by_session_id(session_id.clone()).await.unwrap();
        assert_eq!(by_session.expect("same row").id, id);

        // `?? column` means absent-is-unchanged, which is what lets a four-column
        // patch statement stand in for every transition.
        repo.transition(
            id,
            &status::UNPAID,
            PaymentPatch::succeeded("pi_test".to_string()),
        )
        .await
        .unwrap();

        let got = repo.find_by_booking_id(booking_id).await.unwrap().unwrap();
        assert_eq!(got.status, status::SUCCEEDED);
        assert_eq!(got.intent_id.as_deref(), Some("pi_test"));
        assert_eq!(
            got.amount_cents, 1234,
            "absent columns must survive a patch"
        );
        assert_eq!(got.session_id, session_id, "absent columns must survive");
        assert_eq!(got.refund_id, None);

        // The `WHERE status IN $from` is the guard, so a redelivery is a no-op.
        repo.transition(
            id,
            &status::UNPAID,
            PaymentPatch::failed("declined".to_string()),
        )
        .await
        .unwrap();
        let got = repo.find_by_booking_id(booking_id).await.unwrap().unwrap();
        assert_eq!(
            got.status,
            status::SUCCEEDED,
            "a payment already out of UNPAID must not move"
        );

        drop_row(&db, "payment", id).await;
    }

    /// The riskiest struct in this service: a map column, a `Datetime`, and an
    /// `Option<DateTime<Utc>>` that the patch path has to leave alone.
    #[tokio::test]
    #[ignore]
    async fn a_booking_round_trips_its_map_and_clears_its_hold_on_transition() {
        let db = db().await;
        let repo = BookingMirrorRepository { q: db.clone() };

        let id = Uuid::now_v7();
        let booked = HashMap::from([(
            "2026-08-19".to_string(),
            vec![TimeSlot {
                start: "09:00".to_string(),
                end: "11:00".to_string(),
            }],
        )]);
        let row = BookingMirror {
            id,
            spot_id: Uuid::now_v7(),
            owner_id: Uuid::now_v7(),
            renter_id: Uuid::now_v7(),
            amount_cents: 500,
            booked: booked.clone(),
            status: booking_status::RESERVED.to_string(),
            hold_until: Some(Utc::now() + TimeDelta::minutes(10)),
            ends_at: Utc::now().into(),
            cancel_reason: None,
            release_reason: None,
        };

        repo.upsert(row.clone()).await.unwrap();

        let got = repo.find_by_id(id).await.unwrap().expect("upserted row");
        assert_eq!(got.id, id);
        assert_eq!(
            got.booked, booked,
            "the map column must survive CONTENT $row"
        );
        assert_eq!(got.ends_at, row.ends_at);
        assert!(got.hold_until.is_some());

        // `transition` sets `hold_until = NONE` outside the patch, because `?? column`
        // can leave a value alone but never clear it.
        repo.transition(
            id,
            &[booking_status::RESERVED],
            BookingMirrorPatch::status(booking_status::CONFIRMED),
        )
        .await
        .unwrap();

        let got = repo.find_by_id(id).await.unwrap().unwrap();
        assert_eq!(got.status, booking_status::CONFIRMED);
        assert_eq!(got.hold_until, None, "every transition ends the hold");
        assert_eq!(got.booked, booked, "a patch must not disturb other columns");
        assert_eq!(got.cancel_reason, None);

        drop_row(&db, "booking", id).await;
    }

    #[tokio::test]
    #[ignore]
    async fn payouts_sum_per_owner() {
        let db = db().await;
        let repo = PayoutRepository { q: db.clone() };

        let owner_id = Uuid::now_v7();
        let (a, b) = (Uuid::now_v7(), Uuid::now_v7());
        for (id, amount_cents) in [(a, 700), (b, 300)] {
            repo.upsert(Payout {
                id,
                version: 1,
                owner_id,
                amount_cents,
                created_at: Utc::now().into(),
            })
            .await
            .unwrap();
        }

        assert_eq!(repo.total_for(&owner_id).await.unwrap(), 1000);
        // A host with nothing withdrawn is 0, not an error and not NONE.
        assert_eq!(repo.total_for(&Uuid::now_v7()).await.unwrap(), 0);

        drop_row(&db, "payout", a).await;
        drop_row(&db, "payout", b).await;
    }
}
