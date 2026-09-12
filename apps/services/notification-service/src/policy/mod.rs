//! Decisions, with no I/O in them.
//!
//! Free functions over values: given the state as arguments, say what the answer is.
//! Nothing here opens a connection, reads the environment, or touches a provider —
//! which is what lets every one of them be unit-tested in place, with no `CONFIG`
//! and no network.
//!
//! The line against `client/`: this decides what an email says, `client/` knows how
//! to hand it to Resend. Deliberately no structs — a type with fields here would be
//! state, and state is exactly what this folder does not have.

pub mod template;
