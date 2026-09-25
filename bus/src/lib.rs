//! NATS plumbing shared by every service: connecting, declaring streams, and
//! reporting whether this instance's projections are current enough to serve.
//!
//! Separate from `shared` so that crate stays runtime-free — see bus/Cargo.toml.

pub mod await_version;
pub mod connect;
pub mod health;
pub mod outbox;
pub mod projector;
/// The one table `bus` owns: `_outbox`.
///
/// **Generated**, by the bus pass in `scripts/print-schema.sh` — the doc lives here rather
/// than in the file because `print-schema` would overwrite it there.
///
/// Each of the four write-side databases has an identical copy, created by
/// `migrations/<svc>/0001_init/up.sql`, because every service runs its own outbox relay.
/// It is deliberately kept OUT of `shared::schema` (see the filter in `diesel.toml`):
/// [`outbox::enqueue`] is generic over whichever database its caller holds, so it cannot
/// name a per-service type — and with this declared only here, `bus::schema::_outbox` is
/// the only one that exists. That is what makes "services reach the outbox through `bus`"
/// a compiler rule rather than a convention.
///
/// The generation pass reads every write-side database and refuses to write if they
/// disagree, which is the one thing four copies of the same migration can get wrong.
///
/// `_lease` was here too, for the elected outbox relay. Ordering moved into the data
/// (`projector::decide`), so every replica relays and there is no election left.
pub mod schema;
pub mod service;
pub mod worker;

// `reached` is deliberately not re-exported: `bus::reached(&db, "booking", …)` says
// nothing about what is being reached, while `bus::await_version::reached(…)` does.
pub use await_version::{AWAIT_VERSION, AwaitVersions};
pub use connect::{connect, ensure_streams};
pub use health::Readiness;
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
