//! One repository per domain, each holding the SurrealQL for the statements this
//! service actually issues. Nothing is generated and no trait sits behind them:
//! what a method does is the string in front of you, with no `format!` and no
//! consts spliced in from the domain models.
//!
//! One table here, three statements. Reads select `record::id(id) AS id` plus
//! whatever else comes back in a shape the struct cannot deserialize, then `*`;
//! writes bind the struct whole with `CONTENT $row`. Both mean adding a column to
//! [`shared::domain_models::spot::Spot`] needs no edit here — only a *patchable*
//! column does, in the one `SET` list that names it.
//!
//! Generic over the querier so the same type serves both positions: the service
//! holds one over the pooled `Surreal<Client>`, the projector builds one over the
//! open `&Transaction` for a single event.

pub mod spot_repository;

/// Round-trips the table through a real SurrealDB.
///
/// `#[ignore]`d — needs `spot-service-db` on :8002 with `schemas/spot-schema.surql`
/// imported, and CI runs `cargo test --workspace` with no database:
///
/// ```sh
/// docker compose -f docker/docker-compose-dev.yml up -d spot-service-db
/// cargo test --workspace -- --ignored
/// ```
///
/// The statements above are hand-written strings, and what they rely on cannot be
/// checked any other way: `CONTENT $row` against a SCHEMAFULL table including an
/// `id` the statement also names, and a read that has to come back as something
/// [`shared::domain_models::spot::Spot`] can deserialize — a `geo::Point`, an
/// `Address`, an `Availability`, and two defaulted columns.
#[cfg(test)]
mod live_tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use chrono::Utc;
    use geo::Point;
    use shared::db::Querier;
    use shared::domain_models::spot::{Spot, SpotPatch};
    use shared::general_models::spot::{Address, Availability, TimeSlot, WeeklyAvailability};
    use surrealdb::{Surreal, engine::remote::ws::Client};
    use uuid::Uuid;

    use super::spot_repository::SpotRepository;

    async fn db() -> Arc<Surreal<Client>> {
        Arc::new(
            shared::db::connect("127.0.0.1:8002", "root", "root")
                .await
                .expect("spot-service-db on :8002 — see this module's docs"),
        )
    }

    fn availability() -> Availability {
        Availability {
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
        }
    }

    #[tokio::test]
    #[ignore]
    async fn a_spot_round_trips_and_patches_leave_absent_columns_alone() {
        let db = db().await;
        let repo = SpotRepository { q: db.clone() };

        let id = Uuid::now_v7();
        let owner_id = Uuid::now_v7();
        let row = Spot {
            id,
            owner_id,
            shard: "00".to_string(),
            title: "Driveway".to_string(),
            description: Some("Near the station".to_string()),
            price_per_hour: 250,
            images: vec!["https://example.test/a.jpg".to_string()],
            location: Point::new(4.35, 50.85),
            active: true,
            deleted: false,
            address: Address {
                line1: "Rue 1".to_string(),
                line2: None,
                city: "Brussels".to_string(),
                postal_code: "1000".to_string(),
                region: None,
                country: "BE".to_string(),
                formatted: "Rue 1, 1000 Brussels".to_string(),
            },
            availability: availability(),
            timezone: "Europe/Brussels".to_string(),
            created_at: Utc::now().into(),
            updated_at: Utc::now().into(),
        };

        repo.upsert(row.clone()).await.unwrap();

        let got = repo.find_by_id(id).await.unwrap().expect("upserted row");
        assert_eq!(
            got.id, id,
            "record::id(id) AS id must unwrap the record key"
        );
        // `owner_id` is a plain uuid column, not a link — if the read ever unwrapped
        // it the way it unwraps `id`, this is what would catch it.
        assert_eq!(got.owner_id, owner_id);
        assert_eq!(got.location, Point::new(4.35, 50.85), "geometry round-trip");
        assert_eq!(got.address.formatted, "Rue 1, 1000 Brussels");
        assert_eq!(got.availability.weekly.monday.len(), 1);
        assert_eq!(got.images.len(), 1);
        assert!(!got.deleted);

        // A delete is also a deactivation, and must leave everything else alone.
        repo.patch(id, SpotPatch::deleted(Utc::now()))
            .await
            .unwrap();
        let got = repo.find_by_id(id).await.unwrap().unwrap();
        assert!(got.deleted);
        assert!(!got.active);
        assert_eq!(got.title, "Driveway", "absent columns must survive a patch");
        assert_eq!(got.price_per_hour, 250);
        assert_eq!(got.timezone, "Europe/Brussels");

        db.q("DELETE type::record('spot', $v)")
            .bind(("v", id))
            .await
            .unwrap()
            .check()
            .unwrap();
    }
}
