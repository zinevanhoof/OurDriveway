//! NATS plumbing shared by every service: connecting, declaring streams, and
//! reporting whether this instance's projections are current enough to serve.
//!
//! Separate from `shared` so that crate stays runtime-free — see bus/Cargo.toml.

pub mod await_version;
pub mod connect;
pub mod health;
pub mod lease;
pub mod outbox;
pub mod projector;
pub mod service;
pub mod worker;

// `reached` is deliberately not re-exported: `bus::reached(&db, "booking", …)` says
// nothing about what is being reached, while `bus::await_version::reached(…)` does.
pub use await_version::{AWAIT_VERSION, AwaitVersions};
pub use connect::{connect, ensure_streams};
pub use health::Readiness;
pub use lease::elect;
pub use projector::{Projector, Tx};
pub use worker::Worker;

// `snapshot` is gone. It existed to shortcut a cold start: every projection
// lived on a disposable volume, so a fresh instance replayed the whole log from
// sequence 1 and got slower as the log grew. TiKV is durable and every replica
// of a service now shares one database, so there is no cold start to shortcut —
// a restart resumes from its cursor and a new replica builds nothing.
//
// Durability of the store itself is TiKV's job (BR), not a SurrealDB export, and
// rebuilding a projection is the backfill path rather than a snapshot. Deleting
// it also took the HTTP engine with it: it was the only user of `protocol-http`.
