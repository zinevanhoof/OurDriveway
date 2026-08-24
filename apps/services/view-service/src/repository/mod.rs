//! One repository per table in the read model, each holding the SurrealQL for the
//! statements this service issues. Nothing is generated and no trait sits behind
//! them: what a method does is the string in front of you, with no `format!` and no
//! consts spliced in from the domain models.
//!
//! This service has more statements than any other, and the extra ones are all the
//! same two kinds:
//!
//! - **Link resolution.** Every table but `user` carries a `record<>` link used for
//!   nested GraphQL traversal, set from a subquery that yields NONE when the target
//!   has not been projected yet. Streams have no cross-stream ordering, so this is
//!   the normal case, not an edge one.
//! - **Backfill.** The mirror of the above: when the target finally arrives, it
//!   points the rows that were waiting for it at itself.
//!
//! Both are idempotent, which is what makes replaying or racing another consumer
//! harmless.
//!
//! Which write form a table gets follows from whether it has a link:
//!
//! - `user` has none, so it is written whole with `CONTENT $row` — and it is also
//!   the only table here ever read back whole, which is why it is the only one with
//!   a `find_by_id`. `SELECT *` on any of the others would return an `owner`,
//!   `spot` or `renter` column their structs deliberately do not carry.
//! - `booking` and `payout` are written whole and **relinked in the same
//!   transaction**, because `CONTENT` clears the links first.
//! - `spot` is never written whole: it carries the `owner` link, which USERS
//!   resolves, so its SPOTS writes are a `SET` list naming only the thirteen
//!   SPOTS-owned columns.
//!
//! Each is generic over its querier so the same type serves both positions: the
//! `/me` handler holds one over the pooled `Surreal<Client>`, a projector builds one
//! over the open `&Transaction` for a single event.

pub mod booking_repository;
pub mod payout_repository;
pub mod spot_repository;
pub mod user_repository;

/// Round-trips the read model through a real SurrealDB.
///
/// `#[ignore]`d — needs `view-service-db` on :8003 with `schemas/view-schema.surql`
/// imported, and CI runs `cargo test --workspace` with no database:
///
/// ```sh
/// docker compose -f docker/docker-compose-dev.yml up -d view-service-db
/// cargo test --workspace -- --ignored
/// ```
///
/// The link dance is what these exist for. A `CONTENT $row` write clears `owner`,
/// `spot` and `renter`, and the relink puts them back in the same transaction —
/// a sequence that is correct only if both halves actually run, which no amount of
/// Rust can show. The other half is `spot`, which must never be written whole
/// because `owner` is resolved by a different stream.
#[cfg(test)]
mod live_tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use chrono::Utc;
    use shared::db::Querier;
    use shared::domain_models::booking::status;
    use shared::domain_models::view::{
        ViewBooking, ViewBookingPatch, ViewPayout, ViewSpotPatch, ViewUser, ViewUserPatch,
    };
    use shared::events::booking::ReleaseReason;
    use shared::events::spot::SpotCreated;
    use shared::general_models::spot::{Address, Availability, TimeSlot, WeeklyAvailability};
    use surrealdb::{Surreal, engine::remote::ws::Client};
    use uuid::Uuid;

    use super::booking_repository::ViewBookingRepository;
    use super::payout_repository::ViewPayoutRepository;
    use super::spot_repository::ViewSpotRepository;
    use super::user_repository::ViewUserRepository;

    async fn db() -> Arc<Surreal<Client>> {
        Arc::new(
            shared::db::connect("127.0.0.1:8003", "root", "root")
                .await
                .expect("view-service-db on :8003 — see this module's docs"),
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

    /// Whether the named link column actually points somewhere.
    ///
    /// The `IF … != NONE` is required, not defensive: `record::id(NONE)` is an
    /// *error* in SurrealDB 3.2.4 ("Expected `record` but found `NONE`"), not a NONE
    /// result — and an unresolved link is the normal state this has to observe.
    async fn link_of(db: &Arc<Surreal<Client>>, table: &str, id: Uuid, col: &str) -> Option<Uuid> {
        db.q(format!(
            "SELECT VALUE IF {col} != NONE THEN record::id({col}) ELSE NONE END
             FROM ONLY type::record('{table}', $v)"
        ))
        .bind(("v", id))
        .await
        .unwrap()
        .take(0)
        .unwrap()
    }

    /// A booking's status, for asserting that `settle`'s guard did or did not let a
    /// write through — the signal that used to be its `Option<Uuid>` return.
    async fn status_of(db: &Arc<Surreal<Client>>, id: Uuid) -> String {
        db.q("SELECT VALUE status FROM ONLY type::record('booking', $v)")
            .bind(("v", id))
            .await
            .unwrap()
            .take::<Option<String>>(0)
            .unwrap()
            .expect("the booking row exists")
    }

    fn a_user(id: Uuid) -> ViewUser {
        ViewUser {
            id,
            first_name: "Ada".to_string(),
            last_name: "Lovelace".to_string(),
            profile_picture: None,
            email: Some(format!("view-{id}@example.test")),
            license_plates: vec![],
        }
    }

    fn spot_created(spot_id: Uuid, owner_id: Uuid) -> SpotCreated {
        SpotCreated {
            spot_id,
            owner_id,
            shard: "00".to_string(),
            title: "Driveway".to_string(),
            description: None,
            price_per_hour_cents: 250,
            images: vec![],
            lat: 50.85,
            lng: 4.35,
            address: Address {
                line1: "Rue 1".to_string(),
                line2: None,
                city: "Brussels".to_string(),
                postal_code: "1000".to_string(),
                region: None,
                country: "BE".to_string(),
                formatted: "Rue 1, 1000 Brussels".to_string(),
            },
            availability: Availability {
                weekly: WeeklyAvailability {
                    monday: vec![TimeSlot {
                        start: "08:00".to_string(),
                        end: "18:00".to_string(),
                    }],
                    tuesday: vec![],
                    wednesday: vec![],
                    thursday: vec![],
                    friday: vec![],
                    saturday: vec![],
                    sunday: vec![],
                },
                single: HashMap::new(),
            },
            timezone: "Europe/Brussels".to_string(),
        }
    }

    #[tokio::test]
    #[ignore]
    async fn a_view_user_round_trips_and_patches_leave_absent_columns_alone() {
        let db = db().await;
        let repo = ViewUserRepository { q: db.clone() };

        let id = Uuid::now_v7();
        repo.upsert(a_user(id)).await.unwrap();

        let got = repo.find_by_id(id).await.unwrap().expect("upserted row");
        assert_eq!(got.id, id);
        assert!(got.license_plates.is_empty());

        repo.patch(
            id,
            ViewUserPatch {
                license_plates: Some(vec!["1-ABC-123".to_string()]),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        let got = repo.find_by_id(id).await.unwrap().unwrap();
        assert_eq!(got.license_plates, vec!["1-ABC-123".to_string()]);
        assert_eq!(got.first_name, "Ada", "absent columns must survive a patch");

        drop_row(&db, "user", id).await;
    }

    /// A SPOTS write must fill its own columns and leave `owner` alone, in either
    /// arrival order — which on a read model is the normal case, not an edge one.
    #[tokio::test]
    #[ignore]
    async fn spots_writes_never_disturb_the_link() {
        let db = db().await;
        let spots = ViewSpotRepository { q: db.clone() };
        let users = ViewUserRepository { q: db.clone() };

        let (spot_id, owner_id) = (Uuid::now_v7(), Uuid::now_v7());
        let at = Utc::now();

        // The owner has not been projected yet, so the link cannot resolve.
        spots
            .merge(
                spot_id,
                ViewSpotPatch::created(spot_created(spot_id, owner_id), at),
            )
            .await
            .unwrap();
        spots.link_owner(&spot_id, &owner_id).await.unwrap();
        assert_eq!(
            link_of(&db, "spot", spot_id, "owner").await,
            None,
            "the link must resolve to NONE until the user exists"
        );

        // Now the owner lands, and backfill points every waiting row at them.
        users.upsert(a_user(owner_id)).await.unwrap();
        users.backfill_links(&owner_id).await.unwrap();
        assert_eq!(
            link_of(&db, "spot", spot_id, "owner").await,
            Some(owner_id),
            "backfill must fill the gap the subquery left"
        );

        // A later SPOTS edit must not disturb it.
        spots
            .patch(
                spot_id,
                ViewSpotPatch {
                    active: Some(false),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(
            link_of(&db, "spot", spot_id, "owner").await,
            Some(owner_id),
            "a SPOTS write must not clear the link"
        );

        drop_row(&db, "spot", spot_id).await;
        drop_row(&db, "user", owner_id).await;
    }

    /// `CONTENT` clears the links; `link_refs` puts them back in the same
    /// transaction. Both halves have to run, and only a database can show that.
    #[tokio::test]
    #[ignore]
    async fn a_booking_upsert_clears_then_relinks_its_refs() {
        let db = db().await;
        let bookings = ViewBookingRepository { q: db.clone() };
        let spots = ViewSpotRepository { q: db.clone() };
        let users = ViewUserRepository { q: db.clone() };

        let (booking_id, spot_id, renter_id, owner_id) = (
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
        );
        let at = Utc::now();

        users.upsert(a_user(renter_id)).await.unwrap();
        spots
            .merge(
                spot_id,
                ViewSpotPatch::created(spot_created(spot_id, owner_id), at),
            )
            .await
            .unwrap();

        bookings
            .upsert(ViewBooking {
                id: booking_id,
                spot_id,
                owner_id,
                renter_id,
                booked: HashMap::new(),
                amount: 500,
                status: status::RESERVED.to_string(),
                hold_until: Some(at),
                release_reason: None,
                cancel_reason: None,
                rating: None,
                ends_at: at.into(),
                created_at: at.into(),
            })
            .await
            .unwrap();
        bookings
            .link_refs(&booking_id, &spot_id, &renter_id)
            .await
            .unwrap();

        assert_eq!(
            link_of(&db, "booking", booking_id, "spot").await,
            Some(spot_id)
        );
        assert_eq!(
            link_of(&db, "booking", booking_id, "renter").await,
            Some(renter_id)
        );

        // Settling is guarded: it only leaves the status the event allows.
        bookings
            .settle(booking_id, status::RESERVED, ViewBookingPatch::confirmed())
            .await
            .unwrap();
        assert_eq!(status_of(&db, booking_id).await, status::CONFIRMED);

        // Redelivery: no longer `reserved`, so the guard refuses and the row stands.
        bookings
            .settle(
                booking_id,
                status::RESERVED,
                ViewBookingPatch::released(ReleaseReason::Expired),
            )
            .await
            .unwrap();
        assert_eq!(
            status_of(&db, booking_id).await,
            status::CONFIRMED,
            "the guard must have held"
        );

        // And settling must not have disturbed the links.
        assert_eq!(
            link_of(&db, "booking", booking_id, "spot").await,
            Some(spot_id)
        );
        assert_eq!(
            link_of(&db, "booking", booking_id, "renter").await,
            Some(renter_id)
        );

        drop_row(&db, "booking", booking_id).await;
        drop_row(&db, "spot", spot_id).await;
        drop_row(&db, "user", renter_id).await;
    }

    #[tokio::test]
    #[ignore]
    async fn a_payout_upsert_clears_then_relinks_owner() {
        let db = db().await;
        let payouts = ViewPayoutRepository { q: db.clone() };
        let users = ViewUserRepository { q: db.clone() };

        let (payout_id, owner_id) = (Uuid::now_v7(), Uuid::now_v7());

        payouts
            .upsert(ViewPayout {
                id: payout_id,
                owner_id,
                amount: 700,
                created_at: Utc::now().into(),
            })
            .await
            .unwrap();
        payouts.link_owner(&payout_id, &owner_id).await.unwrap();
        assert_eq!(
            link_of(&db, "payout", payout_id, "owner").await,
            None,
            "no owner projected yet, so the subquery yields NONE"
        );

        // The regression this table's backfill was added for: a payout whose owner
        // had not been projected kept `owner = NONE` permanently, because nothing
        // else ever revisited the row.
        users.upsert(a_user(owner_id)).await.unwrap();
        users.backfill_links(&owner_id).await.unwrap();
        assert_eq!(
            link_of(&db, "payout", payout_id, "owner").await,
            Some(owner_id),
            "backfill must reach payout, not just spot and booking"
        );

        drop_row(&db, "payout", payout_id).await;
        drop_row(&db, "user", owner_id).await;
    }
}
