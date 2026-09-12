//! Decisions, with no I/O in them.
//!
//! Free functions over values: given the state as arguments, say what the answer is.
//! Nothing here opens a connection, publishes an event, or reads the clock — a
//! function that needs the time takes it as a parameter. That is what lets every one
//! of them be unit-tested in place, with no database and no NATS.
//!
//! The line against `service/`: a service knows how to *find* the state and what to
//! do with the answer, this knows what the answer *is*. Deliberately no structs —
//! a type with fields here would be state, and state is exactly what this folder
//! does not have.

pub mod timezone;
