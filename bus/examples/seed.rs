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
//! Publishes events rather than writing to any database, because the log is the
//! source of truth — every service's projection is built from it, so this one
//! command populates all five databases and survives a wipe-and-replay. Seeding
//! SurrealDB directly would fill one database, leave the others empty, and vanish
//! on the next rebuild.
//!
//! Reuses the real event structs on purpose. Hand-written JSON would compile
//! against nothing and drift silently the first time a payload changes.
//!
//! Idempotent, and permanently so. Ids are UUIDv5 derived from the names below,
//! and every seeded entity is published with `expected_last_subject_sequence = 0`
//! — "append only if this subject has never been written". A second run is refused
//! as `Stale` and skipped.
//!
//! The `Nats-Msg-Id` dedupe alone is NOT enough: the stream's duplicate window is
//! 120s, so re-running any later would append a second copy of every event. The
//! projections would survive that (they upsert by the same key) but the log — the
//! actual source of truth — would accumulate a duplicate set on every dev start.

use argon2::{
    Argon2, PasswordHasher,
    password_hash::{SaltString, rand_core::OsRng},
};
use chrono::Utc;
use shared::error::myerror::MyError;
use shared::events::{
    Envelope, shard_of,
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
fn envelope<T>(payload: T, actor_id: Option<Uuid>, event_key: &str) -> Envelope<T> {
    Envelope {
        event_id: stable(event_key),
        occurred_at: Utc::now(),
        actor_id,
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

    for (id, first, last, email) in users {
        let shard = shard_of(&id);
        let subject = user_subject(&shard, &id);

        let registered = envelope(
            UserEvent::Registered(UserRegistered {
                user_id: id,
                shard: shard.clone(),
                first_name: first.into(),
                last_name: last.into(),
                email: email.into(),
                // Hashed here for the same reason the real signup path does it:
                // Argon2 salts randomly, so hashing inside a projection would give
                // every replica a different answer for the same event.
                password_hash: hash(PASSWORD),
            }),
            Some(id),
            &format!("seed:event:registered:{id}"),
        );

        match bus::publish_expecting(&js, subject.clone(), &registered, Some(0)).await {
            Ok(seq) => {
                // Chained on the sequence just returned, not `Some(0)` again — the
                // subject is no longer empty. Without this they could not log in:
                // user-service reads `email_verified` on the same row as the hash.
                let verified = envelope(
                    UserEvent::EmailVerified { user_id: id },
                    Some(id),
                    &format!("seed:event:verified:{id}"),
                );
                bus::publish_expecting(&js, subject, &verified, Some(seq))
                    .await
                    .map_err(MyError::from)?;
                println!("user     {id}  {email}");
            }
            Err(bus::PublishError::Stale) => println!("user     {id}  {email}  (already seeded)"),
            Err(bus::PublishError::Failed(e)) => return Err(e.into()),
        }
    }

    // Owned by Alice, so Bob is the one who can book it — a renter may not book
    // their own spot.
    let (owner_id, ..) = users[0];
    let spot_id = stable("seed:spot:alice-driveway");
    let spot_shard = shard_of(&spot_id);

    let created = envelope(
        SpotEvent::Created(SpotCreated {
            spot_id,
            shard: spot_shard.clone(),
            owner_id,
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
        }),
        Some(owner_id),
        &format!("seed:event:spot-created:{spot_id}"),
    );

    match bus::publish_expecting(&js, spot_subject(&spot_shard, &spot_id), &created, Some(0)).await
    {
        Ok(_) => println!("spot     {spot_id}  owned by {owner_id}"),
        Err(bus::PublishError::Stale) => {
            println!("spot     {spot_id}  owned by {owner_id}  (already seeded)")
        }
        Err(bus::PublishError::Failed(e)) => return Err(e.into()),
    }
    println!("\npassword for both users: {PASSWORD}");
    Ok(())
}
