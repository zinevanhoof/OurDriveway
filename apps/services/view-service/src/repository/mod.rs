//! One repository per table in the read model, each holding the SQL for the
//! statements this service issues. Nothing is generated and no trait sits behind
//! them: what a method does is the string in front of you, with no `format!` and no
//! consts spliced in from the domain models.
//!
//! ## This service got smaller
//!
//! It used to have more statements than any other, and the extra ones were all the
//! same two kinds:
//!
//! - **Link resolution.** Every table but `user` carried a `record<>` link for nested
//!   GraphQL traversal, and a `CONTENT $row` write cleared it — so each write was two
//!   statements, the second putting the link back in the same transaction.
//! - **Backfill.** The mirror of the above: when a link's target finally arrived, it
//!   pointed the rows that had been waiting at itself.
//!
//! Both are gone. `host_id`, `spot_id` and `renter_id` are plain uuid columns with no
//! foreign key — deliberately, because the streams have no cross-stream ordering and a
//! spot is routinely projected before its host. A read LEFT JOINs to resolve them, so
//! a reference to a row that has not arrived is an absent join rather than a null that
//! something has to come back and repair.
//!
//! `spot` keeps its `merge`/`patch` split — only `SpotCreated` may bring a row into
//! existence.
//!
//! ## Where the security lives
//!
//! `migrations/view/0001_init/up.sql` records this as three compound clauses, one per
//! table. That is the shape it had when the permission clauses first moved out of the
//! schema, and the file must not be corrected in place — editing an applied migration
//! means it never runs anywhere it has already run, so the two diverge silently.
//! **This is the current list:**
//!
//! | table | function | namespace | `WHERE` |
//! |---|---|---|---|
//! | `spot` | `find_for_public` | `public` | `id = $1 AND active` |
//! | | `find_pins_for_public` | `public` | `active AND NOT deleted AND host_id <> $1` |
//! | | `find_for_host` | `host` | `id = $1 AND host_id = $2` |
//! | | `find_list_for_host` | `host` | `host_id = $1 AND NOT deleted` |
//! | `booking` | `find_for_public_spot` | `public` | `spot_id = parent AND status IN ('reserved','confirmed')` |
//! | | `find_for_host_spot` | `host` | `spot_id = parent` — ownership already proved |
//! | | `find_list_for_renter` | `renter` | `renter_id = $1` |
//! | | `find_next_for_renter` | `renter` | `renter_id = $1 AND status = 'confirmed' AND ends_at > $2` |
//! | | `find_for_renter` | `renter` | `id = $1 AND renter_id = $2` |
//! | `payment` + `payout` | `wallet::find_month_for_account` | `account` | `host_id = $1 OR renter_id = $1`, as five separately-indexed branches |
//! | | `wallet::find_previous_month_for_account` | `account` | the same, as four `max()`es |
//! | | `wallet::balance_for_host` | `host` | `host_id = $1` |
//! | `app_user` | `find_for_account` | `account` | `id = $1` — the verified claim picks the row |
//!
//! The namespace column is not decoration: it is the route prefix the function is
//! reachable through, and the two must not drift. A read whose `WHERE` does not match its
//! namespace's predicate is either in the wrong namespace or is a second rule nobody
//! wrote down.
//!
//! `payout`'s own `find_all_by_host_id` is gone with the endpoint it served. Its rule
//! — `host_id = $1`, the whole query rather than a clause, because a withdrawal is
//! visible to exactly one person — is now the wallet's payout branch.
//!
//! **`payment` is new here, and it reverses what `0001_init` says.** That file records
//! payments as deliberately absent from the read model. They are present now, because
//! reading is this service's job; what stops the old objection coming true is that no
//! money is ever *spent* against these rows — `PaymentService::request_payout` computes
//! what it pays out from its own tables, in its own transaction, under a lock. See
//! `migrations/view/0003_payment/up.sql`.
//!
//! **One function per audience, rather than one clause serving two.** The old
//! `(active OR host_id = $caller)` and `(renter_id = $caller OR host_id = $caller OR
//! status IN (…))` each answered a stranger and a party from the same statement, with an
//! `is_party` helper cutting the renter, the amount and the hold expiry out of the rows
//! afterwards. Field-level scoping is the *projection type* now
//! (`shared::projections`): a public read does not select what it may not return, so
//! there is nothing in flight to cut and no `Option` that means "denied".
//!
//! ## A parent and its children are two statements
//!
//! `/public/spots/{id}` and `/host/spots/{id}` each read a spot and then the bookings on
//! it, through `belonging_to`. **That is deliberate, not a missing join.** One join would
//! repeat the spot's `images`, `address` and `availability` once per booking, and those
//! are the expensive columns; two statements also give parent and children independent
//! cache keys, so a booking landing invalidates the availability without refetching the
//! listing.
//!
//! A single-statement form does exist and was built and verified during the diesel
//! migration — a correlated `array_agg` over a row constructor, decoded through diesel's
//! `Record<(…)>` type, fully typed. It is not used here for the reasons above.
//!
//! `belonging_to` rather than `spot_id.eq(id)` because the parent row is already in hand:
//! the foreign key is read off the row the first statement authorised, so a second
//! argument cannot name a spot that statement refused.
//!
//! ### `grouped_by` is the reassembly half, and has no caller
//!
//! It is the tool for a *list* of parents each with children —
//! `children.grouped_by(&parents)` gives `Vec<Vec<Child>>` aligned with `parents`, to be
//! zipped. No endpoint here returns that shape: `/host/spots` renders no bookings. When
//! one appears it is one line, and the fallible `try_grouped_by` is the one to reach for —
//! it surfaces orphans instead of dropping them, and this read model has no foreign keys,
//! so a child whose parent has not been projected yet is expected rather than corrupt.
//!
//! ### `BoxableExpression` is not used
//!
//! Nothing in this API filters dynamically: every `WHERE` above is fixed at compile time,
//! including the radius search, whose one runtime input is a bound value rather than a
//! shape. Boxing is the tool for a runtime-assembled predicate — a price range or an
//! availability-day filter on `/public/spots/nearby` — and it costs the prepared-statement
//! cache, because a boxed query has no `QueryId`. Worth it for a filter form; not for
//! these.

pub mod booking_repository;
pub mod payment_repository;
pub mod payout_repository;
pub mod spot_repository;
pub mod user_repository;
pub mod wallet_repository;

/// Round-trips the read model through a real YugabyteDB.
///
/// `#[ignore]`d — needs the dev cluster on :5433, and CI runs
/// `cargo test --workspace` with no database:
///
/// ```sh
/// docker compose -f docker/docker-compose-dev.yml up -d yugabyte
/// cargo test --workspace -- --ignored
/// ```
///
/// **Three tests were deleted here rather than ported**, and what they were for is
/// worth recording. `spots_writes_never_disturb_the_link`,
/// `a_booking_upsert_clears_then_relinks_its_refs` and
/// `a_payout_upsert_clears_then_relinks_host` each existed because a write was two
/// statements that both had to run: the first cleared a record link, the second put it
/// back. Nothing in Rust connected them, so only a database could show the pair was
/// intact. There are no links and no second statements, so there is nothing left for
/// those tests to observe.
///
/// What replaces them is `an_unresolved_reference_is_an_absent_join` — the property
/// those two-statement dances were trying to buy in the first place.
#[cfg(test)]
mod live_tests {
    use std::collections::HashMap;

    use chrono::{Datelike, Timelike, Utc};
    use diesel::prelude::*;
    use diesel_async::RunQueryDsl;
    use shared::domain_models::booking::status;
    use shared::domain_models::view::{
        ViewBooking, ViewBookingPatch, ViewSpotPatch, ViewUser, ViewUserPatch,
    };
    use shared::events::booking::ReleaseReason;
    use shared::events::spot::SpotCreated;
    use shared::general_models::booking::Booked;
    use shared::general_models::spot::{Address, Availability, TimeSlot, WeeklyAvailability};
    use shared::schema::view::{app_user, booking, payment, payout, spot};
    use uuid::Uuid;

    use super::booking_repository::ViewBookingRepository;
    use super::spot_repository::ViewSpotRepository;
    use super::user_repository::ViewUserRepository;

    /// Connects and migrates, so a running container is the only prerequisite.
    async fn db() -> shared::db::Db {
        // SAFETY: tests in one binary share an environment and every caller sets the
        // same value.
        unsafe {
            std::env::set_var(
                "VIEW_DATABASE_URL",
                "postgres://yugabyte@127.0.0.1:5433/view",
            )
        };
        // Through the migrator rather than a second copy of the wiring: that crate is
        // the only thing that migrates in dev and production, so a test cannot drift
        // from what actually gets applied. It creates the database if missing.
        //
        // `ensure` and not `run_one`: test threads run in parallel and would otherwise
        // all try to apply a new migration at once, which YugabyteDB refuses rather than
        // serialises. See the note on that function.
        migrator::ensure("view").await.expect("migrations apply");

        shared::db::connect("postgres://yugabyte@127.0.0.1:5433/view")
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

    fn a_user(id: Uuid) -> ViewUser {
        ViewUser {
            id,
            version: 1,
            first_name: "Ada".to_string(),
            last_name: "Lovelace".to_string(),
            profile_picture: None,
            email: format!("view-{id}@example.test"),
            license_plates: vec![],
            country: None,
        }
    }

    fn spot_created(spot_id: Uuid, host_id: Uuid) -> SpotCreated {
        SpotCreated {
            spot_id,
            host_id,
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

    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn a_view_user_round_trips_and_patches_leave_absent_columns_alone() {
        let pool = db().await;
        let db = &mut *conn(&pool).await;
        let id = Uuid::now_v7();

        ViewUserRepository::upsert(db, a_user(id)).await.unwrap();

        let got = ViewUserRepository::find_for_account(db, id)
            .await
            .unwrap()
            .expect("upserted row");
        assert_eq!(got.public.id, id);
        assert!(got.license_plates.is_empty());

        ViewUserRepository::patch(
            db,
            id,
            ViewUserPatch {
                license_plates: Some(vec!["1-ABC-123".to_string()]),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        let got = ViewUserRepository::find_for_account(db, id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.license_plates, vec!["1-ABC-123".to_string()]);
        assert_eq!(
            got.public.first_name, "Ada",
            "absent columns must survive a patch"
        );
        assert_eq!(got.email, format!("view-{id}@example.test"));

        diesel::delete(app_user::table.find(id))
            .execute(db)
            .await
            .unwrap();
    }

    /// A SPOTS write fills its own columns and leaves the rest alone, in either
    /// arrival order — which on a read model is the normal case, not an edge one.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn a_spots_write_fills_its_own_columns_and_leaves_the_rest() {
        let pool = db().await;
        let db = &mut *conn(&pool).await;
        let (spot_id, host_id) = (Uuid::now_v7(), Uuid::now_v7());
        let at = Utc::now();

        // The host has not been projected yet, and the spot is written anyway. That
        // used to be the hard case — the `host` link could only be resolved against
        // a row that existed — and is now unremarkable: `host_id` is a uuid.
        ViewSpotRepository::merge(
            db,
            spot_id,
            ViewSpotPatch::created(spot_created(spot_id, host_id), at),
        )
        .await
        .unwrap();

        let (got_host, lng, lat): (Uuid, f64, f64) = spot::table
            .find(spot_id)
            .select((spot::host_id, spot::lng, spot::lat))
            .first(db)
            .await
            .unwrap();
        assert_eq!(got_host, host_id);
        assert_eq!((lng, lat), (4.35, 50.85), "lng before lat, both ways");

        // **A `None` on an insert takes the table's default, not NULL.** `Insertable`
        // omits an absent field from the column list, and an omitted column is the only
        // way a default ever applies — a column that is *named* with a NULL bind gets
        // NULL, which is what the `COALESCE($n, default)`s this statement used to carry
        // were compensating for.
        //
        // The three defaulted columns are `images '{}'`, `active true` and
        // `deleted false`. No caller merges a patch this partial today — the projector
        // only ever passes `created()`, which fills all fourteen — so this is the only
        // thing that holds the claim up.
        let bare = Uuid::now_v7();
        ViewSpotRepository::merge(
            db,
            bare,
            ViewSpotPatch {
                images: None,
                active: None,
                deleted: None,
                ..ViewSpotPatch::created(spot_created(bare, host_id), at)
            },
        )
        .await
        .unwrap();

        let (imgs, act, del): (Vec<String>, bool, bool) = spot::table
            .find(bare)
            .select((spot::images, spot::active, spot::deleted))
            .first(db)
            .await
            .unwrap();
        assert!(imgs.is_empty(), "images defaults to '{{}}', not NULL");
        assert!(act, "active defaults to true");
        assert!(!del, "deleted defaults to false");

        diesel::delete(spot::table.find(bare))
            .execute(db)
            .await
            .unwrap();

        // A later SPOTS edit must not disturb what it does not name.
        ViewSpotRepository::patch(
            db,
            spot_id,
            ViewSpotPatch {
                active: Some(false),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        let (active, title, host_after): (bool, String, Uuid) = spot::table
            .find(spot_id)
            .select((spot::active, spot::title, spot::host_id))
            .first(db)
            .await
            .unwrap();
        assert!(!active);
        assert_eq!(title, "Driveway", "COALESCE must hold");
        assert_eq!(host_after, host_id, "an edit must not repoint the host");

        diesel::delete(spot::table.find(spot_id))
            .execute(db)
            .await
            .unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn a_booking_round_trips_and_settle_is_guarded() {
        let pool = db().await;
        let db = &mut *conn(&pool).await;
        let (booking_id, spot_id, renter_id, host_id) = (
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
        );
        let at = Utc::now();

        ViewBookingRepository::upsert(
            db,
            ViewBooking {
                id: booking_id,
                version: 1,
                spot_id,
                host_id,
                renter_id,
                booked: Booked::new(),
                amount: 500,
                status: status::RESERVED.to_string(),
                hold_until: Some(at),
                release_reason: None,
                cancel_reason: None,
                rating: None,
                ends_at: at,
                created_at: at,
            },
        )
        .await
        .unwrap();

        ViewBookingRepository::settle(
            db,
            booking_id,
            status::RESERVED,
            ViewBookingPatch::confirmed(),
        )
        .await
        .unwrap();
        assert_eq!(status_of(db, booking_id).await, status::CONFIRMED);

        // Redelivery: no longer `reserved`, so the guard refuses and the row stands.
        ViewBookingRepository::settle(
            db,
            booking_id,
            status::RESERVED,
            ViewBookingPatch::released(ReleaseReason::Expired),
        )
        .await
        .unwrap();
        assert_eq!(
            status_of(db, booking_id).await,
            status::CONFIRMED,
            "the guard must have held"
        );

        // The unconditional half of `settle`: every transition ends the hold, which a
        // COALESCE patch could never express.
        let hold: Option<chrono::DateTime<Utc>> = booking::table
            .find(booking_id)
            .select(booking::hold_until)
            .first(db)
            .await
            .unwrap();
        assert_eq!(hold, None);

        diesel::delete(booking::table.find(booking_id))
            .execute(db)
            .await
            .unwrap();
    }

    /// What the three deleted link tests were trying to buy, and what
    /// `Option::<T>::as_select()` now has to deliver.
    ///
    /// A booking can be projected before the spot or the renter it names — the streams
    /// advance independently, and this was the case that left a `record<>` link null
    /// *permanently* until it was made unconditional. The read has to degrade to an
    /// absent join rather than dropping the row or failing.
    ///
    /// It goes through the projections rather than raw tuples on purpose: the LEFT JOIN
    /// and the `Option` in the projection are two halves of one claim, and only a real
    /// database shows they agree.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn an_unresolved_reference_is_an_absent_join() {
        let pool = db().await;
        let db = &mut *conn(&pool).await;
        let (booking_id, spot_id, renter_id, host_id) = (
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
        );
        let at = Utc::now();

        // Neither the spot nor the renter exists yet.
        ViewBookingRepository::upsert(
            db,
            ViewBooking {
                id: booking_id,
                version: 1,
                spot_id,
                host_id,
                renter_id,
                booked: Booked::new(),
                amount: 500,
                status: status::RESERVED.to_string(),
                hold_until: None,
                release_reason: None,
                cancel_reason: None,
                rating: None,
                ends_at: at,
                created_at: at,
            },
        )
        .await
        .unwrap();

        // The renter's read LEFT JOINs, so the booking comes back with its spot
        // unresolved rather than not coming back at all.
        let mine = ViewBookingRepository::find_list_for_renter(db, renter_id)
            .await
            .unwrap();
        assert_eq!(mine.len(), 1, "the booking must still be returned");
        assert!(
            mine[0].spot.is_none(),
            "an unprojected spot is an absent join"
        );
        assert!(
            ViewBookingRepository::find_for_renter(db, booking_id, renter_id)
                .await
                .unwrap()
                .is_some(),
            "…and the by-id read is the same join, so it must not drop the row either"
        );

        // The targets arrive. Nothing revisits the booking, and the joins resolve
        // themselves — which is the property the old `link_refs`/`backfill_links` pair
        // spent four methods and three tests approximating.
        ViewUserRepository::upsert(db, a_user(renter_id))
            .await
            .unwrap();
        ViewSpotRepository::merge(
            db,
            spot_id,
            ViewSpotPatch::created(spot_created(spot_id, host_id), at),
        )
        .await
        .unwrap();

        let mine = ViewBookingRepository::find_list_for_renter(db, renter_id)
            .await
            .unwrap();
        let card = mine[0].spot.as_ref().expect("the spot resolves now");
        assert_eq!(card.title, "Driveway");
        assert_eq!(card.id, spot_id);
        assert_eq!(card.timezone, "Europe/Brussels");

        // The other absent join, on the host's side of the same booking. It needs the
        // spot to exist, because the parent read is what the children belong to — which
        // is exactly why this half could not be asserted before the merge above.
        let spot = ViewSpotRepository::find_for_host(db, spot_id, host_id)
            .await
            .unwrap()
            .expect("its host may read it");
        // `at - 1s`, because this booking's `ends_at` **is** `at` and the host read is
        // `ends_at > now`. Passing `at` would return nothing and the assertion below would
        // fail on an index rather than on the join it is about.
        let host_rows =
            ViewBookingRepository::find_for_host_spot(db, &spot, at - chrono::Duration::seconds(1))
                .await
                .unwrap();
        assert_eq!(
            host_rows[0]
                .renter
                .as_ref()
                .expect("the renter resolves now")
                .first_name,
            "Ada"
        );

        diesel::delete(booking::table.find(booking_id))
            .execute(db)
            .await
            .unwrap();
        diesel::delete(spot::table.find(spot_id))
            .execute(db)
            .await
            .unwrap();
        diesel::delete(app_user::table.find(renter_id))
            .execute(db)
            .await
            .unwrap();
    }

    // `a_missing_alias_fails_loudly` is deleted rather than ported.
    //
    // It guarded `MaybeJoined`'s one dangerous edge: it turned a `ColumnDecode` into
    // `None`, so a statement that forgot its `AS user_*` would have hidden every profile
    // on every page with nothing in the logs. There is no aliasing left to forget —
    // diesel matches the select clause by position and checks it against the FROM at
    // compile time — so the failure this proved was loud cannot be written.

    /// The SQL radius filter must return exactly what `policy::geo::haversine` says it
    /// should.
    ///
    /// This is the only thing that checks the trigonometry in
    /// `ViewSpotRepository::find_pins_for_public`. That formula is written out in SQL — six calls
    /// to `radians`, two `power`s, an `asin` — and a transposed `lat`/`lng` or a
    /// dropped `cos` would still return *a* plausible set of spots. Nothing else in the
    /// system would notice; the map would just be subtly wrong.
    ///
    /// It also pins the two-stage design together: the bbox is an index range and the
    /// haversine is the exact filter, so a spot inside the box but outside the circle
    /// must be dropped, and one near the box's corner must not be lost.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn the_sql_radius_agrees_with_the_reference() {
        use crate::policy::geo::haversine;

        let pool = db().await;
        let db = &mut *conn(&pool).await;
        let caller = Uuid::now_v7();
        let host = Uuid::now_v7();
        let (centre_lng, centre_lat) = (4.3517, 50.8466); // Brussels
        let radius = 5_000.0;

        // Spread around the centre, deliberately including points that land inside the
        // bounding box but outside the circle — the corners are what separate the two
        // stages. `ids` keeps them addressable for cleanup.
        let mut ids = Vec::new();
        let mut expected = Vec::new();
        for (i, (d_lng, d_lat)) in [
            (0.0, 0.0),    // dead centre
            (0.01, 0.0),   // ~700m east
            (0.0, 0.03),   // ~3.3km north
            (0.05, 0.0),   // ~3.5km east
            (0.045, 0.04), // inside the BOX corner, outside the CIRCLE
            (0.0, 0.06),   // ~6.7km north — outside
            (0.5, 0.5),    // far outside
        ]
        .into_iter()
        .enumerate()
        {
            let id = Uuid::now_v7();
            let (lng, lat) = (centre_lng + d_lng, centre_lat + d_lat);
            ids.push(id);
            if haversine(centre_lng, centre_lat, lng, lat) <= radius {
                expected.push(id);
            }

            let mut patch = ViewSpotPatch::created(spot_created(id, host), Utc::now());
            patch.lng = Some(lng);
            patch.lat = Some(lat);
            patch.title = Some(format!("spot {i}"));
            ViewSpotRepository::merge(db, id, patch).await.unwrap();
        }

        let got =
            ViewSpotRepository::find_pins_for_public(db, caller, centre_lng, centre_lat, radius)
                .await
                .unwrap();

        let mut got_ids: Vec<Uuid> = got
            .iter()
            .map(|s| s.id)
            .filter(|id| ids.contains(id))
            .collect();
        got_ids.sort();
        expected.sort();
        assert_eq!(
            got_ids, expected,
            "the SQL radius must select exactly what policy::geo::haversine predicts"
        );
        assert!(
            !expected.is_empty() && expected.len() < ids.len(),
            "the fixture must contain both included and excluded spots, or this \
             asserts nothing"
        );

        // The other half of `find_pins_for_public`'s contract: a host is never shown their own
        // driveway as somewhere to park.
        let as_host =
            ViewSpotRepository::find_pins_for_public(db, host, centre_lng, centre_lat, radius)
                .await
                .unwrap();
        assert!(
            !as_host.iter().any(|s| ids.contains(&s.id)),
            "the radius search must always exclude the caller's own spots"
        );

        diesel::delete(spot::table.filter(spot::id.eq_any(&ids)))
            .execute(db)
            .await
            .unwrap();
    }

    /// **The rules that used to be the schema.**
    ///
    /// Every assertion here was a `PERMISSIONS FOR select` clause in
    /// view-schema.surql, enforced by SurrealDB against the browser's own JWT. They are
    /// ordinary WHERE clauses now, which means a dropped one is a silent leak that
    /// compiles — so this is the test that has to exist.
    ///
    /// Read it as the specification: if a rule is not asserted below, nothing enforces
    /// it anywhere.
    ///
    /// It is now organised **one block per function**, because that is how the rules are
    /// organised. There is no shared clause to assert once; each projection is a
    /// different statement, and what makes it safe is which columns it selects and which
    /// rows it matches.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn the_read_model_scopes_what_each_caller_can_see() {
        let pool = db().await;
        let db = &mut *conn(&pool).await;

        let (host, renter, stranger) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
        let (spot_id, booking_id) = (Uuid::now_v7(), Uuid::now_v7());
        let at = Utc::now();
        let ends_at = at + chrono::TimeDelta::hours(4);

        ViewUserRepository::upsert(db, a_user(renter))
            .await
            .unwrap();
        ViewSpotRepository::merge(
            db,
            spot_id,
            ViewSpotPatch::created(spot_created(spot_id, host), at),
        )
        .await
        .unwrap();
        ViewBookingRepository::upsert(
            db,
            ViewBooking {
                id: booking_id,
                version: 1,
                spot_id,
                host_id: host,
                renter_id: renter,
                booked: Booked::new(),
                amount: 4200,
                status: status::RESERVED.to_string(),
                hold_until: Some(at),
                release_reason: None,
                cancel_reason: None,
                rating: None,
                ends_at,
                created_at: at,
            },
        )
        .await
        .unwrap();

        // ── public: the availability answer ──────────────────────────────────
        // A reserved booking IS public, because these rows are what replaced
        // `spot.booked` — a prospective renter has to see that a slot is taken.
        //
        // Read through the parent, as the route does: `belonging_to` takes the foreign
        // key off the row rather than off a second argument, so a caller cannot pass a
        // spot id that the parent statement never authorised.
        let public_spot = ViewSpotRepository::find_for_public(db, spot_id)
            .await
            .unwrap()
            .expect("an active spot is public");
        let public = ViewBookingRepository::find_for_public_spot(db, &public_spot, at)
            .await
            .unwrap();
        assert_eq!(public.len(), 1, "a blocking booking is public by design");

        let b = &public[0];
        assert_eq!(b.id, booking_id, "which booking is public");
        assert_eq!(b.status, status::RESERVED, "and whether it still blocks");
        // Within a microsecond, not equal. `timestamptz` stores microseconds and
        // `Utc::now()` produces nanoseconds, so every timestamp is truncated on the way
        // in. Nothing in the request path compares timestamps for equality — every use
        // is `ends_at > now` or `hold_until < now` — but it is worth one assertion
        // saying so, because the first `assert_eq!` on a round-tripped timestamp
        // anywhere else will fail for a reason that reads like a bug.
        assert!(
            (b.ends_at - ends_at)
                .num_microseconds()
                .unwrap_or(i64::MAX)
                .abs()
                <= 1,
            "the slots' end is public, to the microsecond the column stores"
        );
        // The renter and the amount are not asserted absent here —
        // `PublicBookingProjection` has no such fields, so the compiler is what enforces
        // it and this statement never selects those columns at all.

        // ── host: the host's own rows, in full ───────────────────────────────
        let host_spot = ViewSpotRepository::find_for_host(db, spot_id, host)
            .await
            .unwrap()
            .expect("its host may read it");
        let owned = ViewBookingRepository::find_for_host_spot(db, &host_spot, at)
            .await
            .unwrap();
        assert_eq!(owned.len(), 1);
        assert_eq!(owned[0].amount, 4200, "the host sees the amount");
        assert!(owned[0].renter.is_some(), "the host sees the renter");

        // A released booking stops blocking, so it leaves the availability answer.
        ViewBookingRepository::settle(
            db,
            booking_id,
            status::RESERVED,
            ViewBookingPatch::released(ReleaseReason::Expired),
        )
        .await
        .unwrap();
        assert!(
            ViewBookingRepository::find_for_public_spot(db, &public_spot, at)
                .await
                .unwrap()
                .is_empty(),
            "a released booking must not block a slot"
        );
        assert_eq!(
            ViewBookingRepository::find_for_host_spot(db, &host_spot, at)
                .await
                .unwrap()
                .len(),
            1,
            "…but the host keeps it, which is the history the manage screen shows"
        );

        // ── renter: by id, the renter and nobody else ────────────────────────
        //
        // This is where the old `(renter_id = $2 OR host_id = $2)` disjunction split.
        // The **host** is now among those refused here, and that is not a loss: they read
        // the same booking as a child of their own spot, four assertions above.
        for outsider in [stranger, host] {
            assert!(
                ViewBookingRepository::find_for_renter(db, booking_id, outsider)
                    .await
                    .unwrap()
                    .is_none(),
                "only the renter reads a booking by id"
            );
        }
        let detail = ViewBookingRepository::find_for_renter(db, booking_id, renter)
            .await
            .unwrap()
            .expect("its renter may read it");
        assert_eq!(detail.amount, 4200);
        assert_eq!(detail.id, booking_id);

        // ── renter: the next-up card is one confirmed, unfinished row ────────
        //
        // The booking above is `released` by this point, so there is nothing coming.
        assert!(
            ViewBookingRepository::find_next_for_renter(db, renter, at)
                .await
                .unwrap()
                .is_none(),
            "a released booking is not somewhere the renter is due"
        );

        // ── public vs host (spot) ────────────────────────────────────────────
        ViewSpotRepository::patch(
            db,
            spot_id,
            ViewSpotPatch {
                active: Some(false),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert!(
            ViewSpotRepository::find_for_public(db, spot_id)
                .await
                .unwrap()
                .is_none(),
            "an inactive spot is not a public spot, for anyone"
        );
        assert!(
            ViewSpotRepository::find_for_host(db, spot_id, host)
                .await
                .unwrap()
                .is_some(),
            "…and must still be visible to its host, which is what the switch is for"
        );
        assert!(
            ViewSpotRepository::find_for_host(db, spot_id, stranger)
                .await
                .unwrap()
                .is_none(),
            "a stranger is not the host of anything"
        );

        // ── host: a host's own list excludes deleted ─────────────────────────
        ViewSpotRepository::patch(db, spot_id, ViewSpotPatch::deleted(at))
            .await
            .unwrap();
        assert!(
            !ViewSpotRepository::find_list_for_host(db, host)
                .await
                .unwrap()
                .iter()
                .any(|s| s.id == spot_id),
            "a deleted spot must not appear in its host's list"
        );
        assert!(
            ViewSpotRepository::find_for_host(db, spot_id, host)
                .await
                .unwrap()
                .is_some(),
            "…but the row must survive, so past bookings still resolve a title"
        );

        diesel::delete(booking::table.find(booking_id))
            .execute(db)
            .await
            .unwrap();
        diesel::delete(spot::table.find(spot_id))
            .execute(db)
            .await
            .unwrap();
        diesel::delete(app_user::table.find(renter))
            .execute(db)
            .await
            .unwrap();
    }

    /// The wallet, end to end against a real database: four sources in one ordered
    /// list, the right sign on each, and nothing of anyone else's.
    ///
    /// The signs are the part that only a database can check. Each is a different
    /// branch of one `UNION ALL`, and swapping two of them compiles, passes every unit
    /// test, and shows a host their income as an expense.
    ///
    /// `payout` also had the strictest select rule of any table here —
    /// `FOR select WHERE host_id = record::id($auth)`, with no public half at all — so
    /// the "and nobody else's" half of this is what that rule became.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn a_wallet_month_carries_every_source_with_the_callers_own_signs() {
        use super::payment_repository::ViewPaymentRepository;
        use super::payout_repository::ViewPayoutRepository;
        use super::wallet_repository::WalletRepository;
        use shared::domain_models::payment::payout::status as payout_status;
        use shared::domain_models::view::{ViewPayment, ViewPaymentPatch, ViewPayout};
        use shared::projections::wallet::kind;

        let pool = db().await;
        let db = &mut *conn(&pool).await;
        let (host, renter, stranger) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
        let (spot_id, booking_id) = (Uuid::now_v7(), Uuid::now_v7());
        let (charged, refunded, payout_id) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());

        // Mid-month, so the month's bounds are nowhere near the timestamps and a
        // half-open-range bug shows up as a missing row rather than a lucky pass.
        let at = Utc::now()
            .with_day(15)
            .expect("every month has a 15th")
            .with_hour(12)
            .unwrap();
        let (start, end) =
            crate::policy::wallet::bounds(&crate::policy::wallet::label(at)).unwrap();

        ViewSpotRepository::merge(
            db,
            spot_id,
            ViewSpotPatch::created(spot_created(spot_id, host), at),
        )
        .await
        .unwrap();
        ViewBookingRepository::upsert(
            db,
            ViewBooking {
                id: booking_id,
                version: 1,
                spot_id,
                host_id: host,
                renter_id: renter,
                booked: Booked::new(),
                amount: 2_000,
                status: status::CONFIRMED.to_string(),
                hold_until: None,
                release_reason: None,
                cancel_reason: None,
                rating: None,
                // Already over, so the host's income counts as settled rather than
                // pending — the `pending` assertion below is on this.
                ends_at: at - chrono::Duration::days(2),
                created_at: at,
            },
        )
        .await
        .unwrap();

        for (id, amount) in [(charged, 2_000), (refunded, 700)] {
            ViewPaymentRepository::upsert(
                db,
                ViewPayment {
                    id,
                    version: 1,
                    booking_id,
                    host_id: host,
                    renter_id: renter,
                    amount,
                    status: shared::domain_models::payment::status::SUCCEEDED.to_string(),
                    created_at: at,
                    refunded_at: None,
                },
            )
            .await
            .unwrap();
        }
        ViewPaymentRepository::transition(db, refunded, ViewPaymentPatch::refunded(at))
            .await
            .unwrap();

        ViewPayoutRepository::upsert(
            db,
            ViewPayout {
                id: payout_id,
                version: 1,
                host_id: host,
                amount: 9_900,
                status: payout_status::PAID.to_string(),
                created_at: at,
            },
        )
        .await
        .unwrap();

        // Settled a day ago: everything above is older than that, so nothing is pending.
        let settled_before = at - chrono::Duration::days(1);

        let hosts = WalletRepository::find_month_for_account(db, host, start, end)
            .await
            .unwrap();
        let of = |k: &str, amount: i64| {
            hosts
                .iter()
                .any(|t| t.kind == k && t.amount_cents == amount)
        };

        assert!(of(kind::IN, 2_000), "the charge is income to the host");
        assert!(of(kind::IN, 700), "so is the one later refunded");
        assert!(
            of(kind::REFUND, -700),
            "and giving it back takes it away again"
        );
        assert!(of(kind::PAYOUT, -9_900), "a withdrawal is money leaving");
        // The statement returns the booking's end; whether that counts as pending is
        // `policy::wallet::pending`, which has its own unit tests. What a database is
        // needed for is that `settles_at` arrives at all, and only on a host's charge.
        assert!(
            hosts.iter().all(|t| !crate::policy::wallet::pending(
                t.settles_at,
                t.pending_now,
                settled_before
            )),
            "every booking here ended before the settlement cutoff"
        );
        assert!(
            hosts
                .iter()
                .any(|t| t.kind == kind::IN && t.settles_at.is_some()),
            "a host's charge carries the booking's end, which is what ripens"
        );
        assert!(
            hosts
                .iter()
                .filter(|t| t.kind != kind::IN)
                .all(|t| t.settles_at.is_none()),
            "nothing else can ripen, so nothing else carries an end"
        );
        assert!(
            hosts.iter().any(|t| t.title.as_deref() == Some("Driveway")),
            "the spot join must resolve"
        );

        let renters = WalletRepository::find_month_for_account(db, renter, start, end)
            .await
            .unwrap();
        assert!(
            renters
                .iter()
                .any(|t| t.kind == kind::OUT && t.amount_cents == -2_000),
            "the same charge is an expense to the renter"
        );
        assert!(
            renters
                .iter()
                .any(|t| t.kind == kind::REFUND && t.amount_cents == 700),
            "and their refund is money back"
        );
        assert!(
            !renters.iter().any(|t| t.kind == kind::PAYOUT),
            "a renter must not see the host's withdrawals"
        );

        // Ordering is the whole list's, across the union — not per branch. Sorted in
        // Rust now rather than by SQL, because `ORDER BY` over a `UNION ALL` names a
        // column by position and diesel's `positional_order_by` is not public API.
        assert!(
            hosts
                .windows(2)
                .all(|w| w[0].occurred_at >= w[1].occurred_at),
            "newest first"
        );

        // Two rows off one payment, so the ids have to differ or the list has duplicate
        // keys — which is what the `:kind` suffix is for.
        let ids: Vec<&str> = hosts.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(
            ids.len(),
            ids.iter().collect::<std::collections::HashSet<_>>().len(),
            "every row needs its own id"
        );

        assert!(
            WalletRepository::find_month_for_account(db, stranger, start, end)
                .await
                .unwrap()
                .is_empty(),
            "none of this is anyone else's business"
        );

        // The cursor: nothing older than this month exists for these three.
        assert_eq!(
            WalletRepository::find_newest_before(db, host, start)
                .await
                .unwrap(),
            None,
            "there is no earlier month to walk to"
        );

        // Balance: 2000 earned and settled, 700 refunded (its payment is no longer
        // `succeeded`), 9900 withdrawn.
        let balance = WalletRepository::balance_for_host(db, host, settled_before)
            .await
            .unwrap();
        assert_eq!(balance.earned_cents, 2_000);
        assert_eq!(balance.paid_out_cents, 9_900);
        assert_eq!(
            balance.available_cents, -7_900,
            "overdrawn is the honest answer"
        );
        assert_eq!(balance.pending_cents, 0);

        // Where the withdrawal got to, which is the same question asked twice — once by
        // the list and once by the balance. Both must agree with payment-service's
        // `PayoutRepository::total_for`, which is the authority on the same three
        // statuses.
        for (status, counts, pending) in [
            (payout_status::REQUESTED, true, true),
            (payout_status::PAID, true, false),
            (payout_status::FAILED, false, false),
        ] {
            ViewPayoutRepository::set_status(db, payout_id, status)
                .await
                .unwrap();

            let rows = WalletRepository::find_month_for_account(db, host, start, end)
                .await
                .unwrap();
            let row = rows.iter().find(|t| t.kind == kind::PAYOUT);
            assert_eq!(
                row.is_some(),
                counts,
                "a {status} withdrawal's place in the list"
            );
            // A withdrawal ripens on its status, not on a clock, so it is `pending_now`
            // that carries it — `settles_at` is `None` on every payout.
            assert_eq!(
                row.map(|t| t.pending_now),
                counts.then_some(pending),
                "a {status} withdrawal's PENDING chip"
            );

            let balance = WalletRepository::balance_for_host(db, host, settled_before)
                .await
                .unwrap();
            assert_eq!(
                balance.paid_out_cents,
                if counts { 9_900 } else { 0 },
                "a failed withdrawal must hand the money back — there is no other \
                 mechanism, the balance is derived on every read"
            );
        }

        // ── the cursor and the page must select the same rows ───────────────
        //
        // A payment whose booking has not been projected yet. PAYMENTS and BOOKINGS
        // advance in separate lanes, so this is an ordinary moment rather than a
        // corrupt row — and it is the case where the cursor used to disagree with the
        // page: `find_newest_before` counted it (no booking join) and
        // `find_month_for_account` did not (inner join), so `next_month` could name a
        // month that came back with nothing in it.
        let orphan = Uuid::now_v7();
        let last_month = start - chrono::Duration::days(5);
        ViewPaymentRepository::upsert(
            db,
            ViewPayment {
                id: orphan,
                version: 1,
                booking_id: Uuid::now_v7(), // never projected
                host_id: host,
                renter_id: renter,
                amount: 4_242,
                status: shared::domain_models::payment::status::SUCCEEDED.to_string(),
                created_at: last_month,
                refunded_at: None,
            },
        )
        .await
        .unwrap();

        assert!(
            WalletRepository::find_month_for_account(
                db,
                host,
                last_month - chrono::Duration::days(1),
                last_month + chrono::Duration::days(1),
            )
            .await
            .unwrap()
            .is_empty(),
            "the page cannot render a payment with no booking — it has no slots to show"
        );
        assert!(
            WalletRepository::find_newest_before(db, host, start)
                .await
                .unwrap()
                .is_none(),
            "…so the cursor must not point at it either, or the client is sent to a \
             month that renders empty"
        );

        diesel::delete(payment::table.find(orphan))
            .execute(db)
            .await
            .unwrap();

        diesel::delete(payment::table.filter(payment::id.eq_any([charged, refunded])))
            .execute(db)
            .await
            .unwrap();
        diesel::delete(payout::table.find(payout_id))
            .execute(db)
            .await
            .unwrap();
        diesel::delete(booking::table.find(booking_id))
            .execute(db)
            .await
            .unwrap();
        diesel::delete(spot::table.find(spot_id))
            .execute(db)
            .await
            .unwrap();
    }

    async fn status_of(conn: &mut diesel_async::AsyncPgConnection, id: Uuid) -> String {
        booking::table
            .find(id)
            .select(booking::status)
            .first(conn)
            .await
            .unwrap()
    }
}
