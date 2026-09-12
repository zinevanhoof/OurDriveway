//! One worker per stream, each a decode and a delegation to its service in
//! `crate::service`. Booking mail becomes `bookings.rs` here with
//! `const STREAM = STREAM_BOOKINGS`, paired with `service/booking_worker_service.rs`,
//! and nothing else in the service moves.

pub mod users;
