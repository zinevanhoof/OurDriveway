//! One service per worker, named for it. Booking mail becomes
//! `booking_worker_service.rs` here alongside `worker/bookings.rs`, and nothing else in
//! the service moves.

pub mod user_worker_service;
