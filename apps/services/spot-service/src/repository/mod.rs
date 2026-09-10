//! One repository per domain, each holding the SQL for the statements this service
//! actually issues. Nothing is generated and no trait sits behind them: what a method
//! does is the string in front of you, with no `format!` and no consts spliced in
//! from the domain models. Runtime-checked `query_as`, never `query_as!`.
//!
//! One table here, four statements. Reads are plain `SELECT *` — the record-key
//! unwrapping and the `?? default` fallbacks are gone with the record key and the
//! nullable columns.
//!
//! Writes bind the struct whole. `CONTENT $row` did that under SurrealDB and sqlx had
//! no equivalent, so for a while an insert listed its columns and repeated them under
//! `EXCLUDED` with only the live test below to catch an omission;
//! `#[derive(Insertable, AsChangeset)]` on [`shared::domain_models::spot::Spot`] gives
//! it back, checked against `shared::schema`.
//!
//! The repositories are stateless. They were generic over a `Querier` so one type
//! could serve a service and a projector; every method now takes
//! `&mut AsyncPgConnection`, which is what a pooled connection and an open transaction
//! both are.

pub mod spot_repository;

/// Round-trips the table through a real YugabyteDB.
///
/// `#[ignore]`d — needs the dev cluster on :5433, and CI runs
/// `cargo test --workspace` with no database:
///
/// ```sh
/// docker compose -f docker/docker-compose-dev.yml up -d yugabyte
/// cargo test --workspace -- --ignored
/// ```
///
/// The statements are hand-written strings, and what they rely on cannot be checked
/// any other way: a sixteen-column insert whose `EXCLUDED` list has to match its
/// column list, two `jsonb` columns that have to come back as an `Address` and an
/// `Availability`, and a `text[]`. The one thing that is no longer at risk is the
/// geometry — `location` was a `geo::Point` the driver had to recognise, and it is
/// two `double precision` columns now.
#[cfg(test)]
mod live_tests {
    use std::collections::HashMap;

    use chrono::Utc;
    use diesel::prelude::*;
    use diesel_async::RunQueryDsl;
    use shared::domain_models::spot::{Spot, SpotPatch};
    use shared::general_models::spot::{Address, Availability, TimeSlot, WeeklyAvailability};
    use shared::schema::spot::spot;
    use uuid::Uuid;

    use super::spot_repository::SpotRepository;

    /// Connects and migrates, so a running container is the only prerequisite.
    async fn db() -> shared::db::Db {
        // SAFETY: tests in one binary share an environment and every caller sets the
        // same value.
        unsafe {
            std::env::set_var(
                "SPOT_DATABASE_URL",
                "postgres://yugabyte@127.0.0.1:5433/spot",
            )
        };
        // Through the migrator rather than a second copy of the wiring: that crate is
        // the only thing that migrates in dev and production, so a test cannot drift
        // from what actually gets applied. It creates the database if missing.
        //
        // `ensure` and not `run_one`: test threads run in parallel and would otherwise
        // all try to apply a new migration at once, which YugabyteDB refuses rather than
        // serialises. See the note on that function.
        migrator::ensure("spot").await.expect("migrations apply");

        shared::db::connect("postgres://yugabyte@127.0.0.1:5433/spot")
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

    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn a_spot_round_trips_and_patches_leave_absent_columns_alone() {
        let pool = db().await;
        let db = &mut *conn(&pool).await;

        let id = Uuid::now_v7();
        let host_id = Uuid::now_v7();
        let row = Spot {
            id,
            version: 1,
            host_id,
            title: "Driveway".to_string(),
            description: Some("Near the station".to_string()),
            price_per_hour: 250,
            images: vec!["https://example.test/a.jpg".to_string()],
            lng: 4.35,
            lat: 50.85,
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
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        SpotRepository::upsert(db, row.clone()).await.unwrap();

        let got = SpotRepository::find_by_id(db, id)
            .await
            .unwrap()
            .expect("upserted row");
        assert_eq!(got.id, id);
        assert_eq!(got.version, 1, "the version must survive a whole-row write");
        assert_eq!(got.host_id, host_id);
        assert_eq!(
            (got.lng, got.lat),
            (4.35, 50.85),
            "lng before lat, both ways"
        );
        // The jsonb round-trip, in both directions, through the `jsonb_column!`
        // wrapper on both the serialize and the deserialize side.
        assert_eq!(got.address.formatted, "Rue 1, 1000 Brussels");
        assert_eq!(got.address.line2, None, "an absent optional stays absent");
        assert_eq!(got.availability.weekly.monday.len(), 1);
        assert_eq!(got.availability.weekly.tuesday.len(), 0);
        assert_eq!(got.images.len(), 1);
        assert!(!got.deleted);

        // A delete is also a deactivation, and must leave everything else alone —
        // which is also what would catch a mis-ordered positional bind in `patch`.
        SpotRepository::patch(db, id, SpotPatch::deleted(Utc::now()))
            .await
            .unwrap();
        let got = SpotRepository::find_by_id(db, id).await.unwrap().unwrap();
        assert!(got.deleted);
        assert!(!got.active);
        assert_eq!(got.title, "Driveway", "absent columns must survive a patch");
        assert_eq!(got.price_per_hour, 250);
        assert_eq!(got.timezone, "Europe/Brussels");
        assert_eq!(got.availability.weekly.monday.len(), 1);

        diesel::delete(spot::table.find(id))
            .execute(&mut *conn(&pool).await)
            .await
            .unwrap();
    }
}
