//! One worker per stream. Each is a pure `Event -> Option<Mail>` decision plus a
//! send; booking mail becomes `bookings.rs` here with
//! `const STREAM = STREAM_BOOKINGS` and nothing else in the service moves.

pub mod users;
