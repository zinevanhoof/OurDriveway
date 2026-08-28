//! One repository per table, each holding the SurrealQL for the statements this
//! service actually issues. There is no shared `Repository` trait behind them and
//! nothing is generated: what a method does is the string in front of you, with no
//! `format!` and no consts spliced in from the domain models.
//!
//! Two tables, because this service projects two streams: its own `booking` rows,
//! and a mirror of the spots those bookings are made against. Nine statements
//! between them.
//!
//! The mirror carries no copy of what is booked. Which slots are taken is
//! `BookingRepository::taken_for_spot`, a query over the rows themselves.
//!
//! Reads select `record::id(id) AS id` plus whatever else comes back in a shape the
//! struct cannot deserialize, then `*` — so adding a column to a model needs no
//! edit here.
//!
//! Writes differ per table, and the difference is the whole reason this service has
//! two of them:
//!
//! - `booking` is written whole with `CONTENT $row`. Every column is BOOKINGS-owned,
//!   so there is nothing a full write can erase.
//! - `spot` is a mirror fed by two independent projectors, and carries one column
//!   no SPOTS event knows about. Its SPOTS writes are an `UPSERT … SET col = $col ??
//!   col` list that names only the SPOTS-owned columns; `bookings_seq` is written
//!   solely by `advance`.
//!
//! Each is generic over its querier so the same type serves both positions: the
//! service holds one over the pooled `Surreal<Client>`, a projector builds one
//! over the open `&Transaction` for a single event.

pub mod booking_repository;
pub mod spot_mirror_repository;

/// Round-trips both tables through a real SurrealDB.
///
/// `#[ignore]`d, because these need the shared SurrealDB up on :8000 with
/// `schemas/booking-schema.surql` imported into the `booking` database, and CI
/// runs `cargo test --workspace` with no database:
///
/// ```sh
/// docker compose -f docker/docker-compose-dev.yml up -d surrealdb schema-import
/// cargo test --workspace -- --ignored
/// ```
///
/// They earn their keep because the statements above are hand-written strings, and
/// what they rely on cannot be checked any other way:
///
/// - `CONTENT $row` binds a struct whole against a **SCHEMAFULL** table, including
///   an `id` field the statement also names. SurrealDB requires the two to agree.
/// - A read has to come back as something the struct can deserialize — the record
///   key unwrapped, and on the spot mirror three columns defaulted, which is the
///   *normal* case there rather than an edge one.
/// - `merge` must never touch `bookings_seq`. That is now structural — it is not a
///   field on the patch — but structural in Rust says nothing about what the
///   statement does, and this is the invariant the whole mirror rests on.
///
/// Everything here writes rows under fresh uuids and deletes them again.
#[cfg(test)]
mod live_tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use chrono::{TimeDelta, Utc};
    use shared::db::Querier;
    use shared::domain_models::booking::{Booking, SpotMirrorPatch, status};
    use shared::general_models::spot::{Availability, TimeSlot, WeeklyAvailability};
    use surrealdb::{Surreal, engine::remote::ws::Client};
    use uuid::Uuid;

    use super::booking_repository::BookingRepository;
    use super::spot_mirror_repository::SpotMirrorRepository;

    async fn db() -> Arc<Surreal<Client>> {
        Arc::new(
            shared::db::connect("127.0.0.1:8000", "root", "root", "booking")
                .await
                .expect("shared surrealdb on :8000, db `booking` — see this module's docs"),
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

    fn slots() -> HashMap<String, Vec<TimeSlot>> {
        HashMap::from([(
            "2026-08-19".to_string(),
            vec![TimeSlot {
                start: "09:00".to_string(),
                end: "11:00".to_string(),
            }],
        )])
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

    #[tokio::test]
    #[ignore]
    async fn a_booking_round_trips_and_transitions_are_guarded() {
        let db = db().await;
        let repo = BookingRepository { q: db.clone() };

        let (id, spot_id) = (Uuid::now_v7(), Uuid::now_v7());
        let row = Booking {
            id,
            version: 1,
            spot_id,
            owner_id: Uuid::now_v7(),
            renter_id: Uuid::now_v7(),
            booked: slots(),
            amount: 500,
            status: status::RESERVED.to_string(),
            hold_until: Some(Utc::now() + TimeDelta::minutes(10)),
            release_reason: None,
            cancel_reason: None,
            // Comfortably in the future: `taken_for_spot` floors on `ends_at > now`,
            // so a booking that already ended would correctly not come back.
            ends_at: (Utc::now() + TimeDelta::hours(2)).into(),
            rating: None,
            created_at: Utc::now().into(),
        };

        repo.upsert(row.clone()).await.unwrap();

        let got = repo.find_by_id(id).await.unwrap().expect("upserted row");
        assert_eq!(
            got.id, id,
            "record::id(id) AS id must unwrap the record key"
        );
        assert_eq!(
            got.booked,
            slots(),
            "the map column must survive CONTENT $row"
        );
        assert_eq!(got.amount, 500);
        assert_eq!(got.rating, None);
        assert_eq!(got.ends_at, row.ends_at);
        assert!(got.hold_until.is_some());

        // The transition returns the spot id and clears the hold.
        let moved = repo
            .transition(id, status::CONFIRMED, &[status::RESERVED], None, None)
            .await
            .unwrap();
        assert_eq!(moved, Some(spot_id));

        let got = repo.find_by_id(id).await.unwrap().unwrap();
        assert_eq!(got.status, status::CONFIRMED);
        assert_eq!(got.hold_until, None, "every transition ends the hold");
        assert_eq!(
            got.booked,
            slots(),
            "a transition must not disturb the rest"
        );

        // Redelivery: already out of `reserved`, so the guard refuses and the row is
        // untouched — this is what stops a lapsed-hold event undoing a confirmation
        // that raced it.
        //
        // The spot id still comes back. That is the point of the second statement:
        // the caller has to advance this spot's compare-and-swap cursor even when
        // nothing was written, because the refused event still consumed a subject
        // sequence. Returning `None` here used to strand the cursor behind the
        // subject head and refuse every later reserve on the spot.
        let refused = repo
            .transition(
                id,
                status::RELEASED,
                &[status::RESERVED],
                Some("expired"),
                None,
            )
            .await
            .unwrap();
        assert_eq!(refused, Some(spot_id), "the cursor must still advance");
        let got = repo.find_by_id(id).await.unwrap().unwrap();
        assert_eq!(got.status, status::CONFIRMED, "the guard must have held");
        assert_eq!(got.release_reason, None);

        // What blocks the spot, which is what `spot.booked` used to cache.
        let taken = repo.taken_for_spot(&spot_id, Utc::now()).await.unwrap();
        assert_eq!(taken, slots(), "a confirmed booking blocks its slots");

        // Released and cancelled free the slots again.
        repo.transition(id, status::CANCELLED, &[status::CONFIRMED], None, None)
            .await
            .unwrap();
        let taken = repo.taken_for_spot(&spot_id, Utc::now()).await.unwrap();
        assert!(taken.is_empty(), "a cancelled booking blocks nothing");

        drop_row(&db, "booking", id).await;
    }

    /// The invariant the whole mirror rests on: a SPOTS event must never clobber
    /// the compare-and-swap cursor, in either arrival order.
    #[tokio::test]
    #[ignore]
    async fn spots_writes_never_disturb_the_cursor() {
        let db = db().await;
        let repo = SpotMirrorRepository { q: db.clone() };

        let id = Uuid::now_v7();

        // BOOKINGS side wins the race and creates the row. Every SPOTS column is
        // still absent, which is the normal cold-rebuild case on this table.
        repo.advance(&id, 7).await.unwrap();

        let got = repo
            .find_by_id(id)
            .await
            .unwrap()
            .expect("advance created it");
        assert_eq!(got.id, id);
        assert_eq!(got.bookings_seq, 7);
        assert!(got.owner_id.is_none(), "no SPOTS event has landed yet");
        assert!(
            got.bookable().is_none(),
            "a half-built mirror must fail closed"
        );

        // Now the SPOTS side lands. It must fill its own columns and leave the
        // other two exactly as they were.
        let owner_id = Uuid::now_v7();
        repo.merge(
            id,
            SpotMirrorPatch {
                owner_id: Some(owner_id),
                price_per_hour: Some(700),
                availability: Some(availability()),
                timezone: Some("Europe/Brussels".to_string()),
                active: Some(true),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        let got = repo.find_by_id(id).await.unwrap().unwrap();
        assert_eq!(got.owner_id, Some(owner_id));
        assert_eq!(got.price_per_hour, Some(700));
        assert!(got.bookable().is_some(), "the mirror knows enough now");
        assert_eq!(got.bookings_seq, 7, "merge must not rewind the cursor");

        // A later partial edit must leave everything it does not name alone.
        repo.merge(
            id,
            SpotMirrorPatch {
                active: Some(false),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let got = repo.find_by_id(id).await.unwrap().unwrap();
        assert!(!got.active);
        assert!(!got.deleted, "the live switch must not delete");
        assert_eq!(got.timezone.as_deref(), Some("Europe/Brussels"));
        assert_eq!(got.bookings_seq, 7);

        // `math::max`, the reason advance is its own statement: a redelivered older
        // message must not rewind the cursor, or every later reserve asserts a
        // sequence below the subject's head and is refused forever.
        repo.advance(&id, 3).await.unwrap();
        let got = repo.find_by_id(id).await.unwrap().unwrap();
        assert_eq!(got.bookings_seq, 7, "an older seq must not rewind");

        drop_row(&db, "spot", id).await;
    }
}
