//! Decisions, with no I/O in them.
//!
//! Free functions over values: given the state as arguments, say what the answer is.
//! Nothing here opens a connection, publishes an event, or reads the clock — `in_time`
//! takes `now` as a parameter precisely so it stays in this folder. That is what lets
//! every one of them be unit-tested in place, with no database and no NATS.
//!
//! The line against `service/`: a service knows how to *find* the state and what to
//! do with the answer, this knows what the answer *is*. `BookingService::create_booking`
//! reads the spot and the taken slots, asks [`availability::check`] whether they allow
//! the request, and turns a refusal into a 409 — three jobs, and only the middle one
//! lives here.
//!
//! Deliberately no structs — a type with fields here would be state, and state is
//! exactly what this folder does not have. The enums that do live here
//! ([`availability::Rejection`]) are return values, not holders.

pub mod access;
pub mod availability;
pub mod replay;
pub mod schedule;
