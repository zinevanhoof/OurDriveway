//! Test data: two users and one parking spot, appended to the log.
//!
//!     cargo build --workspace --all-targets && ./target/debug/examples/seed
//!
//! Built with `--workspace`, then run as a plain binary. NOT
//! `cargo run -p bus --example seed`: per CLAUDE.md, `-p` resolves a different
//! feature union than `--workspace` and the two then fight over `target/`,
//! paying a rebuild in both directions on every alternation. `--all-targets` is
//! what extends the workspace build to examples.
//!
//! Writes the authoritative rows AND enqueues their events, exactly as a service
//! does — because that is what a service does now. It used to only publish, and
//! every projection was built from the log; owning services write their own rows
//! today, so a publish-only seed left every database empty and the events with
//! nobody to apply them.
//!
//! So this reaches into user-service's and spot-service's databases directly. That
//! is a thing no *service* may do, and it is deliberate here: the alternative is
//! driving the HTTP API, which would make seeding a spot depend on a live
//! LocationIQ key and a network round trip.
//!
//! The `_outbox` rows are the important half. Each service's relay picks them up
//! and publishes them, which is what still populates view-service and the local
//! mirrors — one command, all five databases, same as before.
//!
//! Reuses the real event structs on purpose. Hand-written JSON would compile
//! against nothing and drift silently the first time a payload changes.
//!
//! Idempotent. Ids are UUIDv5 derived from the names below, so a second run
//! upserts the same rows; the `_outbox` rows are keyed by a v5 event id too, so a
//! re-run replaces rather than appends. The stream's `Nats-Msg-Id` dedupe then
//! discards anything the relay sends twice inside its window.

use argon2::{
    Argon2, PasswordHasher,
    password_hash::{SaltString, rand_core::OsRng},
};
use chrono::Utc;
use diesel_async::scoped_futures::ScopedFutureExt;
use diesel_async::{AsyncConnection, RunQueryDsl};
use shared::domain_models::spot::Spot;
use shared::domain_models::user::User;
use shared::events::{
    Envelope, aggregate_id,
    spot::{SpotCreated, SpotEvent},
    spot_subject,
    user::{UserEvent, UserRegistered},
    user_subject,
};
use shared::general_models::spot::{Address, Availability, TimeSlot, WeeklyAvailability};
use uuid::Uuid;

/// Both seeded users share it. Satisfies the signup rules (upper, digit, special)
/// so it also works if you re-type it into the real form.
const PASSWORD: &str = "Test1234!";

/// A stable id from a name, so re-running seeds the same rows.
fn stable(name: &str) -> Uuid {
    Uuid::new_v5(&Uuid::NAMESPACE_OID, name.as_bytes())
}

/// `Envelope::new` mints a random v7 event id; a seed wants a deterministic one so
/// the broker can recognise a re-run as a duplicate.
fn envelope<T>(
    payload: T,
    actor_id: Option<Uuid>,
    event_key: &str,
    aggregate: String,
    version: i64,
) -> Envelope<T> {
    Envelope {
        event_id: stable(event_key),
        aggregate,
        version,
        occurred_at: Utc::now(),
        actor_id,
        // These *are* the original events, not a re-emission of state derived from
        // them — so notification-service is meant to see a seeded signup and act on
        // it, exactly as it would a real one.
        backfill: false,
        payload,
    }
}

fn hash(password: &str) -> String {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .expect("hash seed password")
        .to_string()
}

/// Mon–Fri 08:00–18:00, weekends closed — enough for the booking form to offer
/// slots without every test having to edit availability first.
fn weekdays_9_to_5() -> Availability {
    let day = || {
        vec![TimeSlot {
            start: "08:00".into(),
            end: "18:00".into(),
        }]
    };
    Availability {
        weekly: WeeklyAvailability {
            monday: day(),
            tuesday: day(),
            wednesday: day(),
            thursday: day(),
            friday: day(),
            saturday: vec![],
            sunday: vec![],
        },
        single: Default::default(),
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://localhost:4222".into());

    // Not `bus::connect`, which fails fast — correct for a service, wrong here.
    // This runs alongside the services from one "run all" command, so it may well
    // start before the broker is accepting connections; waiting is the difference
    // between a seed that works on a cold start and one that races it.
    let client = async_nats::ConnectOptions::new()
        .retry_on_initial_connect()
        .connect(&url)
        .await?;
    let js = async_nats::jetstream::new(client);
    println!("connected to {url}");

    // One URL per database, because each service owns its own — same as the services'
    // own `DATABASE_URL`. Port 5433: YSQL does not listen on the PostgreSQL default.
    let db_url = |name: &str| {
        std::env::var("DATABASE_URL_BASE")
            .unwrap_or_else(|_| "postgres://yugabyte@127.0.0.1:5433".into())
            + "/"
            + name
    };

    // The services declare these too, and `get_or_create_stream` is idempotent —
    // so this also covers being the first thing to reach a fresh broker.
    bus::ensure_streams(&js).await?;

    let users = [
        (
            stable("seed:user:alice"),
            "Alice",
            "Andersen",
            "alice@example.com",
        ),
        (stable("seed:user:bob"), "Bob", "Beckers", "bob@example.com"),
    ];

    let user_db = shared::db::connect(&db_url("user")).await?;

    for (id, first, last, email) in users {
        let registered = UserRegistered {
            user_id: id,
            first_name: first.into(),
            last_name: last.into(),
            email: email.into(),
            // Hashed here for the same reason the real signup path does it: Argon2
            // salts randomly, so it has to happen once on the write side.
            password_hash: hash(PASSWORD),
        };

        let mut conn = user_db.get().await?;

        // Verified on the way in. The real path needs a mailed token, and a seeded
        // account that cannot log in is not a seeded account.
        let mut row = User::registered(registered.clone(), 1);
        row.email_verified = true;
        // The struct is the write. `User` derives `Insertable + AsChangeset`, so
        // `.values(row)` names `app_user`'s columns once — in the model — and this seed
        // holds no second copy of them to go stale. That is what `CONTENT $row` used to
        // do and what sqlx could not, which is why this block was a spelled-out column
        // list for as long as sqlx was here.
        //
        // One transaction: the row and the event that announces it commit together, the
        // same rule every real write path follows.
        let email_for_row = row.email.clone();
        conn.transaction::<(), Box<dyn std::error::Error + Send + Sync>, _>(|conn| {
            let registered = registered.clone();
            async move {
                diesel::insert_into(shared::schema::user::app_user::table)
                    .values(row.clone())
                    .on_conflict(shared::schema::user::app_user::id)
                    .do_update()
                    .set(row)
                    .execute(conn)
                    .await?;

                bus::outbox::enqueue(
                    conn,
                    &user_subject(&id),
                    &envelope(
                        UserEvent::Registered(registered),
                        Some(id),
                        &format!("seed:event:registered:{id}"),
                        aggregate_id("user", &id),
                        1,
                    ),
                )
                .await?;
                Ok(())
            }
            .scope_boxed()
        })
        .await?;

        println!("user     {id}  {}", email_for_row);
    }

    // Owned by Alice, so Bob is the one who can book it — a renter may not book
    // their own spot.
    let (host_id, ..) = users[0];
    let spot_id = stable("seed:spot:alice-driveway");
    let spot_created = SpotCreated {
        spot_id,
        host_id,
        title: "Driveway near Brussels Central".into(),
        description: Some("Seeded test spot. Easy to reach, fits one car.".into()),
        price_per_hour_cents: 250,
        // Empty: real images are R2 object keys uploaded by the browser, and
        // a made-up key would render as a broken image.
        images: vec![],
        lng: 4.3517,
        lat: 50.8466,
        address: Address {
            line1: "Grote Markt 1".into(),
            line2: None,
            city: "Brussels".into(),
            postal_code: "1000".into(),
            region: None,
            country: "Belgium".into(),
            formatted: "Grote Markt 1, 1000 Brussels, Belgium".into(),
        },
        availability: weekdays_9_to_5(),
        // Matches the coordinates above. The real create path derives this
        // from the geocoded point; here it is stated so the seed needs no
        // network call.
        timezone: "Europe/Brussels".into(),
    };

    let spot_db = shared::db::connect(&db_url("spot")).await?;
    let mut conn = spot_db.get().await?;

    let row = Spot::created(spot_created.clone(), Utc::now(), 1);

    // Row and event in one transaction, as above.
    conn.transaction::<(), Box<dyn std::error::Error + Send + Sync>, _>(|conn| {
        let spot_created = spot_created.clone();
        async move {
            diesel::insert_into(shared::schema::spot::spot::table)
                .values(row.clone())
                .on_conflict(shared::schema::spot::spot::id)
                .do_update()
                .set(row)
                .execute(conn)
                .await?;

            bus::outbox::enqueue(
                conn,
                &spot_subject(&spot_id),
                &envelope(
                    SpotEvent::Created(spot_created),
                    Some(host_id),
                    &format!("seed:event:spot-created:{spot_id}"),
                    aggregate_id("spot", &spot_id),
                    1,
                ),
            )
            .await?;
            Ok(())
        }
        .scope_boxed()
    })
    .await?;

    println!("spot     {spot_id}  owned by {host_id}");
    println!("\nRelays publish the outbox rows; view-service catches up within a moment.");
    println!("password for both users: {PASSWORD}");
    Ok(())
}
