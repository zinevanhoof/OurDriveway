//! Demo data for recording the README videos: six people and twenty driveways in
//! Hasselt, with three months of bookings, payments, ratings, a refund and payouts behind
//! them.
//!
//! **Every summary has something to show.** Each driveway has at least one completed,
//! rated booking, so its manage-screen tiles, its public rating and its host's line on the
//! detail sheet are all populated; every host has income both this month and last, so
//! "vs last" has a percentage; one of Sam's driveways is paused, so "active" reads 2/3;
//! and when run before 22:00 one of Sam's driveways is occupied right now.
//!
//!     docker compose -f docker/docker-compose-dev.yml up -d
//!     scripts/migrate.sh
//!     # start all seven services, then:
//!     cargo build --workspace --all-targets && ./target/debug/examples/demo_seed
//!
//! Log in as `sam@example.com` / `Demo1234!` — a host with three driveways, income, a
//! pending balance and two payouts, and a renter with a history, a refund and a booking
//! coming up. Every account below shares that password.
//!
//! **Rows only, then the services' own backfill.** This enqueues no events itself. It
//! writes the authoritative rows into user-, spot-, booking- and
//! payment-service's databases and then calls each one's `POST /internal/backfill`,
//! which re-emits every row as the events that reproduce it — the same path a projection
//! rebuild takes. So the event chains (a cancelled booking is Created, Confirmed,
//! Cancelled; a refunded payment is Created then Refunded) are derived by the services
//! that own them, not restated here to drift.
//!
//! That also makes the history inert. Backfilled envelopes carry `backfill: true`, and
//! `bus::worker` acks those without running a handler: no verification emails, no Stripe
//! refunds against the fake `pi_test_demo_…` intents, no transfers for the seeded
//! payouts. Projectors apply them like any other event.
//!
//! **Run it once, on a fresh stack.** Dates are relative to today, so a re-run on a later
//! day would move rows the read model has already seen at the same version, and the
//! projectors would keep the old copy. It refuses to run twice; reset the stack instead.
//!
//! **Don't cancel a seeded booking on camera.** A live cancel is not a backfill: it wakes
//! the settlement worker, which asks Stripe to refund an intent that does not exist.
//! Cancel something booked during the recording.
//!
//! Coordinates are placed by hand along the named streets — close enough for a map, not
//! survey-grade. The three clusters (Grote Markt, the station, Kolonel Dusartplein) are
//! tight on purpose, so the map groups them until you zoom in.

use std::collections::HashMap;

use argon2::{
    Argon2, PasswordHasher,
    password_hash::{SaltString, rand_core::OsRng},
};
use chrono::{
    DateTime, Datelike, Duration, NaiveDate, NaiveTime, TimeZone, Timelike, Utc, Weekday,
};
use chrono_tz::Europe::Brussels;
use diesel::{OptionalExtension, QueryDsl};
use diesel_async::RunQueryDsl;
use serde::Deserialize;
use shared::domain_models::booking::{Booking, status as booking_status};
use shared::domain_models::payment::{Payment, Payout, payout, status as payment_status};
use shared::domain_models::spot::Spot;
use shared::domain_models::user::User;
use shared::events::booking::{CancelReason, ReleaseReason};
use shared::events::{spot::SpotCreated, user::UserRegistered};
use shared::general_models::booking::Booked;
use shared::general_models::spot::{Address, Availability, TimeSlot, WeeklyAvailability};
use shared::schema::{booking::booking, payment::payment, payment::payout as payout_table};
use shared::schema::{spot::spot, user::app_user};
use uuid::Uuid;

type Error = Box<dyn std::error::Error + Send + Sync>;

/// Spot photos: whole URLs of images already uploaded to the R2 bucket, under `spots/`.
///
/// They must pass `shared::media::is_media_url`, the check a real upload meets:
/// `{MEDIA_BASE}/spots/<32 hex chars>.<ext>` — name each file with
/// `uuidgen | tr -d -` before uploading. A URL of any other shape would still display,
/// but editing that spot on camera would then fail validation. Checked before anything
/// is written.
///
/// One per spot, round-robin, so neighbouring spots never share a photo. An empty list
/// works, but the edit form requires at least one photo, so a seeded spot could not be
/// saved.
const SPOT_PHOTOS: &[&str] = &[
    "https://pub-840a7b1ce14341a5ba374ccba35a2c6f.r2.dev/spots/6cda5af46c115fc4afe3c74cbb207286.jpg",
    "https://pub-840a7b1ce14341a5ba374ccba35a2c6f.r2.dev/spots/f420da83d3935c72a24c59af10569aea.webp",
    "https://pub-840a7b1ce14341a5ba374ccba35a2c6f.r2.dev/spots/f5ddc5a10181578592e20a61549729dd.webp",
    "https://pub-840a7b1ce14341a5ba374ccba35a2c6f.r2.dev/spots/f81440ae90bf5ccf857b73f4f4c56271.jpg",
    "https://pub-840a7b1ce14341a5ba374ccba35a2c6f.r2.dev/spots/442e833db66f5cb88282edfa9f0bff5e.webp",
    "https://pub-840a7b1ce14341a5ba374ccba35a2c6f.r2.dev/spots/ce42ddec216157c8bc5b4474bec58dc8.webp",
];

/// Profile pictures, same rules under `avatars/`. Handed out in [`PEOPLE`] order; anyone
/// past the end of the list shows their initials.
const AVATARS: &[&str] = &[];

const PASSWORD: &str = "Demo1234!";

/// `(key, first name, last name, licence plates)`. Email is `<key>@example.com`.
///
/// Sam is the account the videos are recorded as. Everyone has a country, because
/// Stripe will not open a payout account without one.
const PEOPLE: [(&str, &str, &str, &[&str]); 6] = [
    ("sam", "Sam", "Janssens", &["1-SAM-742"]),
    ("lotte", "Lotte", "Peeters", &["1-LPT-318"]),
    ("jonas", "Jonas", "Claes", &["2-JCL-904"]),
    ("lina", "Lina", "Jacobs", &["1-LJA-551"]),
    ("emma", "Emma", "Maes", &["1-EMM-260", "2-EMA-114"]),
    ("noah", "Noah", "Wouters", &["1-NWO-837"]),
];

#[derive(Clone, Copy)]
enum Hours {
    /// Every day between the two times.
    Daily(&'static str, &'static str),
    /// Weekdays only — someone who drives to work and leaves the driveway empty.
    Weekdays(&'static str, &'static str),
    /// Weekday evenings and all weekend — someone who is home during the week.
    EveningsAndWeekends,
}

struct SpotSeed {
    key: &'static str,
    host: &'static str,
    title: &'static str,
    description: &'static str,
    /// EUR cents per hour.
    price: i64,
    line1: &'static str,
    postal_code: &'static str,
    lat: f64,
    lng: f64,
    hours: Hours,
    listed_days_ago: i64,
}

const SPOTS: &[SpotSeed] = &[
    // ── Grote Markt cluster ─────────────────────────────────────────────────
    SpotSeed {
        key: "havermarkt",
        host: "lotte",
        title: "Gated courtyard off the Havermarkt",
        description: "Behind a gate, two minutes' walk from the Grote Markt. Fits a family car; the gate code is sent once you book.",
        price: 350,
        line1: "Havermarkt 12",
        postal_code: "3500",
        lat: 50.9299,
        lng: 5.3368,
        hours: Hours::Daily("07:00", "22:00"),
        listed_days_ago: 120,
    },
    SpotSeed {
        key: "zuivelmarkt",
        host: "sam",
        title: "Driveway on the Zuivelmarkt",
        description: "Short, flat driveway right in the old town. Ideal for an afternoon of shopping or a night out, and free around the clock.",
        price: 300,
        line1: "Zuivelmarkt 8",
        postal_code: "3500",
        lat: 50.9312,
        lng: 5.3372,
        hours: Hours::Daily("00:00", "23:30"),
        listed_days_ago: 110,
    },
    SpotSeed {
        key: "botermarkt",
        host: "lotte",
        title: "Covered spot near the Botermarkt",
        description: "Under a carport, so your car stays dry. Free on weekdays while I'm at the office.",
        price: 400,
        line1: "Botermarkt 5",
        postal_code: "3500",
        lat: 50.9309,
        lng: 5.3389,
        hours: Hours::Weekdays("08:00", "18:00"),
        listed_days_ago: 95,
    },
    SpotSeed {
        key: "kapelstraat",
        host: "lotte",
        title: "Garage driveway in the Kapelstraat",
        description: "In front of the garage, off a quiet side street in the shopping district.",
        price: 380,
        line1: "Kapelstraat 21",
        postal_code: "3500",
        lat: 50.9302,
        lng: 5.3384,
        hours: Hours::Daily("08:00", "20:00"),
        listed_days_ago: 80,
    },
    SpotSeed {
        key: "hoogstraat",
        host: "jonas",
        title: "Tandem driveway, Hoogstraat",
        description: "Room for one car in front of mine. Available in the evenings and all weekend.",
        price: 320,
        line1: "Hoogstraat 30",
        postal_code: "3500",
        lat: 50.9296,
        lng: 5.3380,
        hours: Hours::EveningsAndWeekends,
        listed_days_ago: 60,
    },
    // ── Station cluster ─────────────────────────────────────────────────────
    SpotSeed {
        key: "stationsplein",
        host: "lotte",
        title: "Commuter spot by the station",
        description: "Park, walk two minutes and catch your train. Popular with people heading to Brussels and Leuven.",
        price: 250,
        line1: "Stationsplein 4",
        postal_code: "3500",
        lat: 50.9262,
        lng: 5.3285,
        hours: Hours::Daily("06:00", "22:00"),
        listed_days_ago: 100,
    },
    SpotSeed {
        key: "bampslaan",
        host: "jonas",
        title: "Quiet driveway on the Bampslaan",
        description: "Tree-lined street between the station and the ring road. Easy in, easy out.",
        price: 220,
        line1: "Bampslaan 18",
        postal_code: "3500",
        lat: 50.9272,
        lng: 5.3302,
        hours: Hours::Daily("07:00", "21:00"),
        listed_days_ago: 90,
    },
    SpotSeed {
        key: "roppesingel",
        host: "lina",
        title: "Wide driveway on the ring road",
        description: "Straight off the Groene Boulevard, wide enough for a van.",
        price: 200,
        line1: "Gouverneur Roppesingel 60",
        postal_code: "3500",
        lat: 50.9253,
        lng: 5.3310,
        hours: Hours::Weekdays("07:00", "19:00"),
        listed_days_ago: 70,
    },
    // ── Kolonel Dusartplein cluster ─────────────────────────────────────────
    SpotSeed {
        key: "dusartplein",
        host: "lotte",
        title: "Carport at the Kolonel Dusartplein",
        description: "Covered parking by the square, handy for the restaurants and the cinema.",
        price: 300,
        line1: "Kolonel Dusartplein 3",
        postal_code: "3500",
        lat: 50.9291,
        lng: 5.3412,
        hours: Hours::Daily("08:00", "23:30"),
        listed_days_ago: 85,
    },
    SpotSeed {
        key: "kunstlaan",
        host: "jonas",
        title: "Driveway behind the cultural centre",
        description: "Two minutes from CCHA. Free from mid-morning until late, so perfect for an evening show.",
        price: 280,
        line1: "Kunstlaan 7",
        postal_code: "3500",
        lat: 50.9284,
        lng: 5.3431,
        hours: Hours::Daily("10:00", "23:30"),
        listed_days_ago: 75,
    },
    SpotSeed {
        key: "luikersteenweg",
        host: "lina",
        title: "EV-ready driveway, Luikersteenweg",
        description: "Wall charger available on request. Free day and night.",
        price: 350,
        line1: "Luikersteenweg 45",
        postal_code: "3500",
        lat: 50.9270,
        lng: 5.3440,
        hours: Hours::Daily("00:00", "23:30"),
        listed_days_ago: 50,
    },
    // ── Around town ─────────────────────────────────────────────────────────
    SpotSeed {
        key: "kempische",
        host: "sam",
        title: "Family driveway near the Kapermolenpark",
        description: "A short walk from the park and the city centre. Plenty of room to open the doors.",
        price: 180,
        line1: "Kempische Steenweg 120",
        postal_code: "3500",
        lat: 50.9395,
        lng: 5.3405,
        hours: Hours::Daily("07:00", "21:00"),
        listed_days_ago: 105,
    },
    SpotSeed {
        key: "corda",
        host: "jonas",
        title: "Business-park spot at Corda Campus",
        description: "Right next to the Corda offices. Weekdays only.",
        price: 200,
        line1: "Kempische Steenweg 293",
        postal_code: "3500",
        lat: 50.9500,
        lng: 5.3515,
        hours: Hours::Weekdays("07:00", "19:00"),
        listed_days_ago: 45,
    },
    SpotSeed {
        key: "kanaalkom",
        host: "lotte",
        title: "Waterside driveway at the Kanaalkom",
        description: "Overlooking the canal basin and the Blauwe Boulevard. Evenings and weekends.",
        price: 300,
        line1: "Aldestraat 10",
        postal_code: "3500",
        lat: 50.9368,
        lng: 5.3262,
        hours: Hours::EveningsAndWeekends,
        listed_days_ago: 40,
    },
    SpotSeed {
        key: "salvator",
        host: "lina",
        title: "Driveway near the hospital",
        description: "For visitors to the Salvator campus — much cheaper than the hospital car park.",
        price: 200,
        line1: "Salvatorstraat 20",
        postal_code: "3500",
        lat: 50.9380,
        lng: 5.3290,
        hours: Hours::Daily("07:00", "21:00"),
        listed_days_ago: 65,
    },
    SpotSeed {
        key: "martelarenlaan",
        host: "sam",
        title: "Student-friendly spot by UHasselt",
        description: "Opposite the old prison campus of UHasselt. Cheap all-day parking for students and staff.",
        price: 150,
        line1: "Martelarenlaan 40",
        postal_code: "3500",
        lat: 50.9273,
        lng: 5.3360,
        hours: Hours::Daily("07:00", "22:00"),
        listed_days_ago: 100,
    },
    SpotSeed {
        key: "kuringen",
        host: "jonas",
        title: "Driveway in Kuringen",
        description: "Quiet village street, ten minutes from the centre by bike. Early starts welcome.",
        price: 150,
        line1: "Kuringersteenweg 250",
        postal_code: "3511",
        lat: 50.9480,
        lng: 5.3035,
        hours: Hours::Daily("06:00", "23:30"),
        listed_days_ago: 30,
    },
    SpotSeed {
        key: "sint-truiden",
        host: "jonas",
        title: "Driveway on the Sint-Truidersteenweg",
        description: "On the main road into town from the south. Weekdays while I'm at work.",
        price: 180,
        line1: "Sint-Truidersteenweg 150",
        postal_code: "3500",
        lat: 50.9200,
        lng: 5.3245,
        hours: Hours::Weekdays("07:00", "19:00"),
        listed_days_ago: 25,
    },
    SpotSeed {
        key: "genkersteenweg",
        host: "lina",
        title: "Driveway on the Genkersteenweg",
        description: "East side of town, quick access to the E313. Evenings and weekends.",
        price: 170,
        line1: "Genkersteenweg 200",
        postal_code: "3500",
        lat: 50.9440,
        lng: 5.3600,
        hours: Hours::EveningsAndWeekends,
        listed_days_ago: 20,
    },
    SpotSeed {
        key: "kiewit",
        host: "jonas",
        title: "Green driveway in Kiewit",
        description: "Next to the Kiewit nature reserve — park here for a walk in the woods.",
        price: 150,
        line1: "Putvennestraat 110",
        postal_code: "3500",
        lat: 50.9655,
        lng: 5.3560,
        hours: Hours::Daily("08:00", "20:00"),
        listed_days_ago: 15,
    },
];

#[derive(Clone, Copy)]
enum Fate {
    /// Paid and kept.
    Confirmed,
    /// Paid, then cancelled by the renter this many days after booking — and refunded.
    Cancelled { after_days: i64 },
    /// Left at checkout without paying.
    Abandoned,
}

struct BookingSeed {
    key: &'static str,
    spot: &'static str,
    renter: &'static str,
    /// Days from today, in Brussels. Negative is in the past.
    day: i64,
    slots: &'static [(&'static str, &'static str)],
    /// How long before `day` it was booked. At least one.
    booked_days_before: i64,
    fate: Fate,
}

const BOOKINGS: &[BookingSeed] = &[
    // ── Sam's driveways: three months of income, and two renters still to come ──
    BookingSeed {
        key: "b01",
        spot: "zuivelmarkt",
        renter: "emma",
        day: -84,
        slots: &[("13:00", "17:00")],
        booked_days_before: 5,
        fate: Fate::Confirmed,
    },
    BookingSeed {
        key: "b02",
        spot: "kempische",
        renter: "noah",
        day: -71,
        slots: &[("09:00", "12:30")],
        booked_days_before: 2,
        fate: Fate::Confirmed,
    },
    BookingSeed {
        key: "b03",
        spot: "martelarenlaan",
        renter: "lina",
        day: -63,
        slots: &[("08:00", "12:00"), ("13:00", "17:00")],
        booked_days_before: 7,
        fate: Fate::Confirmed,
    },
    BookingSeed {
        key: "b04",
        spot: "zuivelmarkt",
        renter: "noah",
        day: -52,
        slots: &[("19:00", "23:30")],
        booked_days_before: 1,
        fate: Fate::Confirmed,
    },
    BookingSeed {
        key: "b05",
        spot: "kempische",
        renter: "emma",
        day: -41,
        slots: &[("10:00", "16:00")],
        booked_days_before: 3,
        fate: Fate::Confirmed,
    },
    BookingSeed {
        key: "b06",
        spot: "zuivelmarkt",
        renter: "lina",
        day: -33,
        slots: &[("12:00", "15:00")],
        booked_days_before: 4,
        fate: Fate::Confirmed,
    },
    BookingSeed {
        key: "b07",
        spot: "martelarenlaan",
        renter: "emma",
        day: -24,
        slots: &[("08:30", "17:30")],
        booked_days_before: 6,
        fate: Fate::Confirmed,
    },
    BookingSeed {
        key: "b08",
        spot: "zuivelmarkt",
        renter: "noah",
        day: -15,
        slots: &[("18:00", "22:00")],
        booked_days_before: 2,
        fate: Fate::Confirmed,
    },
    BookingSeed {
        key: "b09",
        spot: "kempische",
        renter: "lina",
        day: -8,
        slots: &[("09:00", "13:00")],
        booked_days_before: 3,
        fate: Fate::Confirmed,
    },
    BookingSeed {
        key: "b10",
        spot: "zuivelmarkt",
        renter: "emma",
        day: -2,
        slots: &[("14:00", "18:00")],
        booked_days_before: 5,
        fate: Fate::Confirmed,
    },
    BookingSeed {
        key: "b11",
        spot: "zuivelmarkt",
        renter: "emma",
        day: 1,
        slots: &[("09:00", "12:00")],
        booked_days_before: 4,
        fate: Fate::Confirmed,
    },
    BookingSeed {
        key: "b12",
        spot: "martelarenlaan",
        renter: "noah",
        day: 4,
        slots: &[("08:00", "16:00")],
        booked_days_before: 2,
        fate: Fate::Confirmed,
    },
    BookingSeed {
        key: "b13",
        spot: "kempische",
        renter: "noah",
        day: -20,
        slots: &[("10:00", "12:00")],
        booked_days_before: 1,
        fate: Fate::Abandoned,
    },
    // ── Sam as a renter: a history, a refund, and the next trip on the home screen ──
    BookingSeed {
        key: "b20",
        spot: "havermarkt",
        renter: "sam",
        day: -58,
        slots: &[("11:00", "15:00")],
        booked_days_before: 3,
        fate: Fate::Confirmed,
    },
    BookingSeed {
        key: "b21",
        spot: "dusartplein",
        renter: "sam",
        day: -37,
        slots: &[("19:00", "23:30")],
        booked_days_before: 1,
        fate: Fate::Confirmed,
    },
    BookingSeed {
        key: "b22",
        spot: "stationsplein",
        renter: "sam",
        day: -12,
        slots: &[("07:00", "18:00")],
        booked_days_before: 5,
        fate: Fate::Confirmed,
    },
    BookingSeed {
        key: "b23",
        spot: "kunstlaan",
        renter: "sam",
        day: -18,
        slots: &[("19:30", "23:00")],
        booked_days_before: 10,
        fate: Fate::Cancelled { after_days: 4 },
    },
    BookingSeed {
        key: "b24",
        spot: "stationsplein",
        renter: "sam",
        day: 2,
        slots: &[("07:30", "18:30")],
        booked_days_before: 6,
        fate: Fate::Confirmed,
    },
    // ── Everyone else, so other spots show taken slots and hosts have renters ──
    BookingSeed {
        key: "b30",
        spot: "havermarkt",
        renter: "emma",
        day: 1,
        slots: &[("10:00", "13:00")],
        booked_days_before: 2,
        fate: Fate::Confirmed,
    },
    BookingSeed {
        key: "b31",
        spot: "kapelstraat",
        renter: "lina",
        day: 3,
        slots: &[("09:00", "17:00")],
        booked_days_before: 5,
        fate: Fate::Confirmed,
    },
    BookingSeed {
        key: "b32",
        spot: "bampslaan",
        renter: "noah",
        day: -30,
        slots: &[("08:00", "18:00")],
        booked_days_before: 3,
        fate: Fate::Confirmed,
    },
    BookingSeed {
        key: "b33",
        spot: "luikersteenweg",
        renter: "emma",
        day: -10,
        slots: &[("20:00", "23:30")],
        booked_days_before: 1,
        fate: Fate::Confirmed,
    },
    BookingSeed {
        key: "b34",
        spot: "kuringen",
        renter: "noah",
        day: 6,
        slots: &[("06:00", "09:00")],
        booked_days_before: 1,
        fate: Fate::Confirmed,
    },
    BookingSeed {
        key: "b35",
        spot: "salvator",
        renter: "lina",
        day: -5,
        slots: &[("14:00", "17:00")],
        booked_days_before: 2,
        fate: Fate::Confirmed,
    },
    // ── So every driveway has a completed booking, and every host income both this
    //    month and last — every summary tile and rating has something to show ──
    //
    // Weekday-only spots are safe to book on any offset: `open_day` moves a booking to
    // the nearest day the spot is open. Evening slots sit inside "evenings and weekends"
    // whichever day they land on. Every booking is after its spot was listed.
    BookingSeed {
        key: "b40",
        spot: "havermarkt",
        renter: "noah",
        day: -40,
        slots: &[("10:00", "14:00")],
        booked_days_before: 3,
        fate: Fate::Confirmed,
    },
    BookingSeed {
        key: "b41",
        spot: "havermarkt",
        renter: "jonas",
        day: -6,
        slots: &[("16:00", "19:00")],
        booked_days_before: 2,
        fate: Fate::Confirmed,
    },
    BookingSeed {
        key: "b42",
        spot: "botermarkt",
        renter: "emma",
        day: -35,
        slots: &[("09:00", "17:00")],
        booked_days_before: 4,
        fate: Fate::Confirmed,
    },
    BookingSeed {
        key: "b43",
        spot: "botermarkt",
        renter: "noah",
        day: -9,
        slots: &[("08:00", "12:00")],
        booked_days_before: 2,
        fate: Fate::Confirmed,
    },
    BookingSeed {
        key: "b44",
        spot: "botermarkt",
        renter: "sam",
        day: 5,
        slots: &[("09:00", "15:00")],
        booked_days_before: 3,
        fate: Fate::Confirmed,
    },
    BookingSeed {
        key: "b45",
        spot: "kapelstraat",
        renter: "noah",
        day: -26,
        slots: &[("11:00", "15:00")],
        booked_days_before: 2,
        fate: Fate::Confirmed,
    },
    BookingSeed {
        key: "b46",
        spot: "kapelstraat",
        renter: "jonas",
        day: -4,
        slots: &[("13:00", "18:00")],
        booked_days_before: 1,
        fate: Fate::Confirmed,
    },
    BookingSeed {
        key: "b47",
        spot: "stationsplein",
        renter: "emma",
        day: -48,
        slots: &[("06:30", "19:00")],
        booked_days_before: 3,
        fate: Fate::Confirmed,
    },
    BookingSeed {
        key: "b48",
        spot: "dusartplein",
        renter: "lina",
        day: -14,
        slots: &[("18:00", "23:00")],
        booked_days_before: 2,
        fate: Fate::Confirmed,
    },
    BookingSeed {
        key: "b49",
        spot: "kanaalkom",
        renter: "noah",
        day: -22,
        slots: &[("19:00", "22:00")],
        booked_days_before: 3,
        fate: Fate::Confirmed,
    },
    BookingSeed {
        key: "b50",
        spot: "kanaalkom",
        renter: "sam",
        day: -3,
        slots: &[("19:00", "22:30")],
        booked_days_before: 1,
        fate: Fate::Confirmed,
    },
    BookingSeed {
        key: "b51",
        spot: "hoogstraat",
        renter: "emma",
        day: -19,
        slots: &[("19:00", "22:00")],
        booked_days_before: 2,
        fate: Fate::Confirmed,
    },
    BookingSeed {
        key: "b52",
        spot: "hoogstraat",
        renter: "lotte",
        day: -2,
        slots: &[("19:30", "23:00")],
        booked_days_before: 1,
        fate: Fate::Confirmed,
    },
    BookingSeed {
        key: "b53",
        spot: "bampslaan",
        renter: "lotte",
        day: -7,
        slots: &[("08:00", "17:00")],
        booked_days_before: 2,
        fate: Fate::Confirmed,
    },
    BookingSeed {
        key: "b54",
        spot: "kunstlaan",
        renter: "emma",
        day: -44,
        slots: &[("19:00", "23:00")],
        booked_days_before: 5,
        fate: Fate::Confirmed,
    },
    BookingSeed {
        key: "b55",
        spot: "kunstlaan",
        renter: "lina",
        day: -8,
        slots: &[("19:30", "23:00")],
        booked_days_before: 2,
        fate: Fate::Confirmed,
    },
    BookingSeed {
        key: "b56",
        spot: "corda",
        renter: "noah",
        day: -29,
        slots: &[("08:00", "18:00")],
        booked_days_before: 3,
        fate: Fate::Confirmed,
    },
    BookingSeed {
        key: "b57",
        spot: "corda",
        renter: "lotte",
        day: -11,
        slots: &[("08:30", "17:30")],
        booked_days_before: 2,
        fate: Fate::Confirmed,
    },
    BookingSeed {
        key: "b58",
        spot: "corda",
        renter: "emma",
        day: 2,
        slots: &[("08:00", "16:00")],
        booked_days_before: 2,
        fate: Fate::Confirmed,
    },
    BookingSeed {
        key: "b59",
        spot: "kuringen",
        renter: "emma",
        day: -21,
        slots: &[("07:00", "12:00")],
        booked_days_before: 2,
        fate: Fate::Confirmed,
    },
    BookingSeed {
        key: "b60",
        spot: "sint-truiden",
        renter: "lina",
        day: -20,
        slots: &[("08:00", "17:00")],
        booked_days_before: 2,
        fate: Fate::Confirmed,
    },
    BookingSeed {
        key: "b61",
        spot: "sint-truiden",
        renter: "sam",
        day: -6,
        slots: &[("07:30", "12:30")],
        booked_days_before: 1,
        fate: Fate::Confirmed,
    },
    BookingSeed {
        key: "b62",
        spot: "kiewit",
        renter: "noah",
        day: -9,
        slots: &[("09:00", "13:00")],
        booked_days_before: 2,
        fate: Fate::Confirmed,
    },
    BookingSeed {
        key: "b63",
        spot: "kiewit",
        renter: "lotte",
        day: 8,
        slots: &[("10:00", "16:00")],
        booked_days_before: 2,
        fate: Fate::Confirmed,
    },
    BookingSeed {
        key: "b64",
        spot: "roppesingel",
        renter: "jonas",
        day: -33,
        slots: &[("08:00", "18:00")],
        booked_days_before: 3,
        fate: Fate::Confirmed,
    },
    BookingSeed {
        key: "b65",
        spot: "roppesingel",
        renter: "emma",
        day: -13,
        slots: &[("09:00", "15:00")],
        booked_days_before: 2,
        fate: Fate::Confirmed,
    },
    BookingSeed {
        key: "b66",
        spot: "luikersteenweg",
        renter: "noah",
        day: -38,
        slots: &[("08:00", "20:00")],
        booked_days_before: 4,
        fate: Fate::Confirmed,
    },
    BookingSeed {
        key: "b67",
        spot: "salvator",
        renter: "jonas",
        day: -31,
        slots: &[("10:00", "15:00")],
        booked_days_before: 2,
        fate: Fate::Confirmed,
    },
    BookingSeed {
        key: "b68",
        spot: "genkersteenweg",
        renter: "sam",
        day: -12,
        slots: &[("19:00", "22:00")],
        booked_days_before: 2,
        fate: Fate::Confirmed,
    },
    BookingSeed {
        key: "b69",
        spot: "genkersteenweg",
        renter: "noah",
        day: 3,
        slots: &[("19:00", "23:00")],
        booked_days_before: 1,
        fate: Fate::Confirmed,
    },
];

/// `(booking key, stars)`: what the renter rated a booking that happened.
///
/// Most completed bookings are rated, so every driveway and every host has a rating to
/// show. A few recent ones are left unrated, which is how real ratings trail. Checked
/// below to name only confirmed bookings that are over — a rating on anything else would
/// be dropped by the projectors anyway.
const RATINGS: &[(&str, i32)] = &[
    ("b01", 5),
    ("b02", 4),
    ("b03", 5),
    ("b04", 4),
    ("b05", 5),
    ("b06", 3),
    ("b07", 5),
    ("b08", 4),
    ("b20", 5),
    ("b21", 4),
    ("b22", 4),
    ("b32", 4),
    ("b33", 5),
    ("b35", 5),
    ("b40", 5),
    ("b41", 4),
    ("b42", 4),
    ("b43", 5),
    ("b45", 3),
    ("b46", 5),
    ("b47", 4),
    ("b48", 5),
    ("b49", 4),
    ("b50", 5),
    ("b51", 4),
    ("b52", 5),
    ("b53", 4),
    ("b54", 5),
    ("b55", 4),
    ("b56", 3),
    ("b57", 4),
    ("b59", 5),
    ("b60", 4),
    ("b61", 5),
    ("b62", 5),
    ("b64", 4),
    ("b65", 5),
    ("b66", 4),
    ("b67", 5),
    ("b68", 4),
];

/// A listing its host has switched off, so "active parking spots" reads 2/3 for Sam.
/// Its past bookings still count; it just takes no new ones.
const PAUSED: &[&str] = &["kempische"];

/// The driveway occupied while the demo is recorded: a two-hour booking starting on the
/// current hour, today, so Sam's "booked right now" has something to count. Open around
/// the clock, and nothing else is booked on it today.
const LIVE: (&str, &str, &str) = ("b70", "zuivelmarkt", "lina");

/// `(key, host, EUR cents, days ago)`. Kept below what the host had earned by then —
/// checked below — so the balance never goes negative.
const PAYOUTS: &[(&str, &str, i64, i64)] = &[("p1", "sam", 4_000, 45), ("p2", "sam", 2_500, 14)];

/// Upserts a whole row, keyed on its primary key.
macro_rules! upsert {
    ($conn:expr, $table:expr, $id:expr, $row:expr) => {{
        let row = $row;
        diesel::insert_into($table)
            .values(row.clone())
            .on_conflict($id)
            .do_update()
            .set(row)
            .execute($conn)
            .await?;
    }};
}

/// A stable id, so every run derives the same ones.
fn stable(name: &str) -> Uuid {
    Uuid::new_v5(&Uuid::NAMESPACE_OID, format!("demo:{name}").as_bytes())
}

fn person(key: &str) -> Uuid {
    stable(&format!("user:{key}"))
}

/// The car a renter turns up in: their first plate, the one the booking form offers
/// first. Everyone in [`PEOPLE`] has at least one, and a demo renter without a plate
/// could not have made this booking on the real path either.
fn plate(key: &str) -> String {
    // `.iter().next()` rather than `.first()`: diesel's `FirstDsl` is in scope here and
    // wins the method lookup on a slice.
    PEOPLE
        .iter()
        .find(|(k, ..)| *k == key)
        .and_then(|(.., plates)| plates.iter().next())
        .unwrap_or_else(|| panic!("no person named {key}"))
        .to_string()
}

fn spot_seed(key: &str) -> &'static SpotSeed {
    SPOTS
        .iter()
        .find(|s| s.key == key)
        .unwrap_or_else(|| panic!("no spot named {key}"))
}

/// A Stripe-shaped handle that no Stripe account has ever issued.
fn fake_stripe(prefix: &str, id: &Uuid) -> String {
    format!("{prefix}_test_demo_{}", &id.simple().to_string()[..16])
}

fn availability(hours: Hours) -> Availability {
    let slot = |start: &str, end: &str| {
        vec![TimeSlot {
            start: start.into(),
            end: end.into(),
        }]
    };
    let (weekday, weekend) = match hours {
        Hours::Daily(start, end) => (slot(start, end), slot(start, end)),
        Hours::Weekdays(start, end) => (slot(start, end), vec![]),
        Hours::EveningsAndWeekends => (slot("18:00", "23:30"), slot("00:00", "23:30")),
    };
    Availability {
        weekly: WeeklyAvailability {
            monday: weekday.clone(),
            tuesday: weekday.clone(),
            wednesday: weekday.clone(),
            thursday: weekday.clone(),
            friday: weekday,
            saturday: weekend.clone(),
            sunday: weekend,
        },
        single: Default::default(),
    }
}

/// A booking with its date settled and its slots owned — what the writing loop consumes.
struct Plan {
    key: &'static str,
    spot: &'static SpotSeed,
    renter: &'static str,
    date: NaiveDate,
    slots: Vec<(String, String)>,
    booked_days_before: i64,
    fate: Fate,
}

/// `today + day`, moved to the nearest date the spot is open for every slot — earlier
/// for a past booking, later for a future one, so neither crosses today.
///
/// Dates move with the day the seed runs, so a fixed offset on a weekday-only driveway
/// lands on a weekend some runs. This keeps the booking instead of failing the seed.
fn open_day(hours: Hours, today: NaiveDate, day: i64, slots: &[(String, String)]) -> NaiveDate {
    let step = if day < 0 { -1 } else { 1 };
    (0..7)
        .map(|shift| today + Duration::days(day + shift * step))
        .find(|date| {
            slots
                .iter()
                .all(|(start, end)| covers(hours, *date, (start, end)))
        })
        .unwrap_or_else(|| panic!("no day within a week is open for {slots:?}"))
}

/// Today's slot for [`LIVE`]: from the current hour for two hours, in Hasselt, capped at
/// 23:30 when the driveway closes. `None` from 22:00, when too little of the day is left
/// for a booking that is still happening while the demo is recorded.
fn live_slot(now: DateTime<Utc>) -> Option<(String, String)> {
    let hour = now.with_timezone(&Brussels).hour();
    (hour < 22).then(|| {
        let end = if hour + 2 >= 23 {
            "23:30".to_string()
        } else {
            format!("{:02}:00", hour + 2)
        };
        (format!("{hour:02}:00"), end)
    })
}

/// Whether the spot is open for the whole of `(start, end)` on `date`. Dates move with
/// today, so a booking on a weekday-only spot would land on a weekend some runs — this
/// makes that a loud failure instead of a booking outside the host's hours.
fn covers(hours: Hours, date: NaiveDate, (start, end): (&str, &str)) -> bool {
    let weekly = availability(hours).weekly;
    let day = match date.weekday() {
        Weekday::Mon => weekly.monday,
        Weekday::Tue => weekly.tuesday,
        Weekday::Wed => weekly.wednesday,
        Weekday::Thu => weekly.thursday,
        Weekday::Fri => weekly.friday,
        Weekday::Sat => weekly.saturday,
        Weekday::Sun => weekly.sunday,
    };
    // `HH:MM` sorts as text, so plain string comparison is time comparison.
    day.iter()
        .any(|open| open.start.as_str() <= start && end <= open.end.as_str())
}

fn time(hhmm: &str) -> NaiveTime {
    NaiveTime::parse_from_str(hhmm, "%H:%M").expect("slot times are HH:MM")
}

/// A wall-clock time in Hasselt, as an instant.
fn local(date: NaiveDate, hhmm: &str) -> DateTime<Utc> {
    Brussels
        .from_local_datetime(&date.and_time(time(hhmm)))
        .earliest()
        .expect("no seeded slot falls in the DST gap")
        .with_timezone(&Utc)
}

fn hash(password: &str) -> String {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .expect("hash demo password")
        .to_string()
}

/// Fails before anything is written if a photo URL would not survive the edit form.
fn check_photos() -> Result<(), Error> {
    if SPOT_PHOTOS.is_empty() {
        println!(
            "note: SPOT_PHOTOS is empty — spots will have no photos, and the edit form needs one"
        );
    }
    if SPOT_PHOTOS.is_empty() && AVATARS.is_empty() {
        return Ok(());
    }

    // The same value spot-service validates against, read from its own .env unless the
    // environment already says otherwise.
    let base = match std::env::var("MEDIA_BASE") {
        Ok(base) => base,
        Err(_) => dotenvy::from_filename_iter("apps/services/spot-service/.env")?
            .filter_map(Result::ok)
            .find(|(key, _)| key == "MEDIA_BASE")
            .map(|(_, value)| value)
            .ok_or("MEDIA_BASE is not set and not in apps/services/spot-service/.env")?,
    };
    shared::media::init_base(&base);

    for (urls, prefix) in [
        (SPOT_PHOTOS, shared::media::PREFIX_SPOTS),
        (AVATARS, shared::media::PREFIX_AVATARS),
    ] {
        if let Some(bad) = urls
            .iter()
            .find(|url| !shared::media::is_media_url(url, prefix))
        {
            return Err(format!("{bad} is not a {base}/{prefix}/<32 hex chars>.<ext> URL").into());
        }
    }
    Ok(())
}

/// One photo per spot, round-robin over [`SPOT_PHOTOS`].
fn photos_for(index: usize) -> Vec<String> {
    SPOT_PHOTOS
        .get(index % SPOT_PHOTOS.len().max(1))
        .map(|url| vec![url.to_string()])
        .unwrap_or_default()
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    check_photos()?;

    let db_url = |name: &str| {
        std::env::var("DATABASE_URL_BASE")
            .unwrap_or_else(|_| "postgres://yugabyte@127.0.0.1:5433".into())
            + "/"
            + name
    };
    let mut users = shared::db::connect(&db_url("user"))
        .await?
        .get_owned()
        .await?;
    let mut spots = shared::db::connect(&db_url("spot"))
        .await?
        .get_owned()
        .await?;
    let mut bookings = shared::db::connect(&db_url("booking"))
        .await?
        .get_owned()
        .await?;
    let mut payments = shared::db::connect(&db_url("payment"))
        .await?
        .get_owned()
        .await?;

    let already = app_user::table
        .find(person("sam"))
        .select(app_user::id)
        .first::<Uuid>(&mut *users)
        .await
        .optional()?;
    if already.is_some() {
        return Err("the demo data is already there. It runs once per fresh stack: \
                    `docker compose -f docker/docker-compose-dev.yml down -v`, up, \
                    scripts/migrate.sh, start the services, then run this again. To only \
                    re-send the events: curl -XPOST localhost:<3000|3001|3002|3006>/internal/backfill"
            .into());
    }

    let now = Utc::now();
    let today = now.with_timezone(&Brussels).date_naive();

    // ── people ──────────────────────────────────────────────────────────────
    // One hash for everyone: same password, and Argon2 is slow in a debug build.
    let password_hash = hash(PASSWORD);
    for (i, (key, first, last, plates)) in PEOPLE.iter().enumerate() {
        let id = person(key);
        let mut row = User::registered(
            UserRegistered {
                user_id: id,
                first_name: first.to_string(),
                last_name: last.to_string(),
                email: format!("{key}@example.com"),
            },
            1,
            password_hash.clone(),
        );
        row.email_verified = true;
        row.license_plates = plates.iter().map(|p| p.to_string()).collect();
        row.country = Some("BE".into());
        row.profile_picture = AVATARS.get(i).map(|url| url.to_string());
        upsert!(&mut *users, app_user::table, app_user::id, row);
    }
    println!("people    {}", PEOPLE.len());

    // ── driveways ───────────────────────────────────────────────────────────
    for (i, s) in SPOTS.iter().enumerate() {
        let created = SpotCreated {
            spot_id: stable(&format!("spot:{}", s.key)),
            host_id: person(s.host),
            title: s.title.into(),
            description: Some(s.description.into()),
            price_per_hour_cents: s.price,
            images: photos_for(i),
            lng: s.lng,
            lat: s.lat,
            address: Address {
                line1: s.line1.into(),
                line2: None,
                city: "Hasselt".into(),
                postal_code: s.postal_code.into(),
                region: Some("Limburg".into()),
                country: "Belgium".into(),
                // The shape spot-service's autocomplete builds, so a seeded address reads
                // like one a host picked from the suggestions.
                formatted: format!("{}, {} Hasselt, Limburg, Belgium", s.line1, s.postal_code),
            },
            availability: availability(s.hours),
            timezone: "Europe/Brussels".into(),
        };
        let mut row = Spot::created(created, now - Duration::days(s.listed_days_ago), 1);
        if PAUSED.contains(&s.key) {
            // Created, then switched off: the two events the backfill replays for it.
            row.active = false;
            row.version = 2;
        }
        upsert!(&mut *spots, spot::table, spot::id, row);
    }
    println!("spots     {}", SPOTS.len());

    // ── bookings, and the payment behind each paid one ──────────────────────
    // Settled income per host, to hold the payouts to.
    let mut earned: HashMap<Uuid, i64> = HashMap::new();

    // Every booking, with its date settled and its slots owned: the constant ones moved to
    // a day their spot is open, plus the live one built from the clock.
    let mut plans: Vec<Plan> = BOOKINGS
        .iter()
        .map(|b| {
            let s = spot_seed(b.spot);
            let slots: Vec<(String, String)> = b
                .slots
                .iter()
                .map(|&(start, end)| (start.to_string(), end.to_string()))
                .collect();
            Plan {
                key: b.key,
                spot: s,
                renter: b.renter,
                date: open_day(s.hours, today, b.day, &slots),
                slots,
                booked_days_before: b.booked_days_before,
                fate: b.fate,
            }
        })
        .collect();

    match live_slot(now) {
        Some(slot) => {
            let (key, spot, renter) = LIVE;
            plans.push(Plan {
                key,
                spot: spot_seed(spot),
                renter,
                date: today,
                slots: vec![slot],
                booked_days_before: 1,
                fate: Fate::Confirmed,
            });
        }
        None => println!("note: too late in the day for a booking happening right now"),
    }

    let ratings: HashMap<&str, i32> = RATINGS.iter().copied().collect();

    for b in &plans {
        let s = b.spot;
        let date = b.date;
        for (start, end) in &b.slots {
            assert!(
                covers(s.hours, date, (start, end)),
                "{} books {start}–{end} on {date}, outside {}'s hours",
                b.key,
                s.key
            );
        }

        let minutes: i64 = b
            .slots
            .iter()
            .map(|(start, end)| (time(end) - time(start)).num_minutes())
            .sum();
        // The same arithmetic `create_booking` uses: truncating, in the renter's favour.
        let amount = minutes * s.price / 60;
        let ends_at = local(date, &b.slots.last().expect("a booking has slots").1);
        // The evening before, never later than an hour ago — an upcoming booking was
        // still made in the past.
        let created_at = local(date - Duration::days(b.booked_days_before), "20:00")
            .min(now - Duration::hours(1));

        let rating = ratings.get(b.key).copied();
        if let Some(stars) = rating {
            assert!(
                matches!(b.fate, Fate::Confirmed) && ends_at < now && (1..=5).contains(&stars),
                "{} is rated {stars}, but only a confirmed booking that is over can be",
                b.key
            );
        }

        // Versions are the length of each chain `replay::events` rebuilds from the row:
        // a rating is one more event after the confirmation.
        let (status, version, cancel_reason, release_reason) = match b.fate {
            Fate::Confirmed => (
                booking_status::CONFIRMED,
                if rating.is_some() { 3 } else { 2 },
                None,
                None,
            ),
            Fate::Cancelled { .. } => (
                booking_status::CANCELLED,
                3,
                Some(CancelReason::ByRenter),
                None,
            ),
            Fate::Abandoned => (
                booking_status::RELEASED,
                2,
                None,
                Some(ReleaseReason::Abandoned),
            ),
        };

        let booking_id = stable(&format!("booking:{}", b.key));
        let host_id = person(s.host);
        let renter_id = person(b.renter);
        let booked: Booked = [(
            date.format("%Y-%m-%d").to_string(),
            b.slots
                .iter()
                .map(|(start, end)| TimeSlot {
                    start: start.clone(),
                    end: end.clone(),
                })
                .collect(),
        )]
        .into_iter()
        .collect();

        upsert!(
            &mut *bookings,
            booking::table,
            booking::id,
            Booking {
                id: booking_id,
                version,
                spot_id: stable(&format!("spot:{}", s.key)),
                host_id,
                renter_id,
                booked,
                license_plate: plate(b.renter),
                amount,
                status: status.into(),
                // Cleared by every transition out of `reserved`, as on the live path.
                hold_until: None,
                release_reason: release_reason.map(|r| r.as_str().to_string()),
                cancel_reason: cancel_reason.map(|r| r.as_str().to_string()),
                ends_at,
                rating,
                created_at,
            }
        );

        // An abandoned checkout never paid, so it has no payment to show.
        let (paid_status, payment_version, refunded_at) = match b.fate {
            Fate::Confirmed => (payment_status::SUCCEEDED, 2, None),
            Fate::Cancelled { after_days } => {
                let refunded_at = created_at + Duration::days(after_days);
                let starts_at = local(date, &b.slots[0].0);
                assert!(
                    refunded_at <= now && refunded_at < starts_at - Duration::hours(1),
                    "{} is refunded after the cancel deadline",
                    b.key
                );
                (payment_status::REFUNDED, 3, Some(refunded_at))
            }
            Fate::Abandoned => continue,
        };

        let payment_id = stable(&format!("payment:{}", b.key));
        upsert!(
            &mut *payments,
            payment::table,
            payment::id,
            Payment {
                id: payment_id,
                version: payment_version,
                booking_id,
                host_id,
                renter_id,
                amount_cents: amount,
                session_id: fake_stripe("cs", &payment_id),
                intent_id: Some(fake_stripe("pi", &payment_id)),
                status: paid_status.into(),
                refund_id: refunded_at.map(|_| fake_stripe("re", &payment_id)),
                refunded_at,
                failure_reason: None,
                created_at: created_at + Duration::minutes(2),
            }
        );

        if paid_status == payment_status::SUCCEEDED && ends_at < now {
            *earned.entry(host_id).or_default() += amount;
        }
    }
    println!("bookings  {} ({} rated)", plans.len(), RATINGS.len());

    // ── payouts ─────────────────────────────────────────────────────────────
    let mut paid_out: HashMap<Uuid, i64> = HashMap::new();
    for &(key, host, cents, days_ago) in PAYOUTS {
        let id = stable(&format!("payout:{key}"));
        let host_id = person(host);
        *paid_out.entry(host_id).or_default() += cents;

        upsert!(
            &mut *payments,
            payout_table::table,
            payout_table::id,
            Payout {
                id,
                // Requested, then paid.
                version: 2,
                host_id,
                amount_cents: cents,
                status: payout::status::PAID.into(),
                transfer_id: Some(fake_stripe("tr", &id)),
                failure_reason: None,
                created_at: now - Duration::days(days_ago),
            }
        );
    }
    for (host_id, cents) in &paid_out {
        let earned = earned.get(host_id).copied().unwrap_or(0);
        assert!(
            *cents <= earned,
            "{host_id} is paid out {cents} but only earned {earned}"
        );
    }
    println!("payouts   {}", PAYOUTS.len());

    // ── events ──────────────────────────────────────────────────────────────
    // Each writing service re-emits its rows. The read model and every mirror build
    // themselves from those, the same way a projection rebuild does.
    #[derive(Deserialize)]
    struct Backfilled {
        events: usize,
    }

    let client = reqwest::Client::new();
    let mut missed = vec![];
    for (service, port) in [
        ("user-service", 3000),
        ("spot-service", 3002),
        ("booking-service", 3001),
        ("payment-service", 3006),
    ] {
        let url = format!("http://localhost:{port}/internal/backfill");
        match client
            .post(&url)
            .send()
            .await
            .and_then(|res| res.error_for_status())
        {
            Ok(res) => println!(
                "{service:<16} re-emitted {} events",
                res.json::<Backfilled>().await?.events
            ),
            Err(e) => {
                println!("{service:<16} unreachable: {e}");
                missed.push(url);
            }
        }
    }

    if !missed.is_empty() {
        println!("\nThe rows are written, but some services were not running. Start them, then:");
        for url in missed {
            println!("  curl -XPOST {url}");
        }
    }

    println!("\nlog in as sam@example.com / {PASSWORD} (every demo account shares it)");
    Ok(())
}
