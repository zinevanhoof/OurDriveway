//! NATS plumbing shared by every service: connecting, declaring streams, and
//! reporting whether this instance's projections are current enough to serve.
//!
//! Separate from `shared` so that crate stays runtime-free — see bus/Cargo.toml.

pub mod await_seq;
pub mod connect;
pub mod health;
pub mod projector;
pub mod publisher;
pub mod service;
pub mod snapshot;
pub mod worker;

pub use await_seq::{AWAIT_SEQ, AppliedSeqs, await_applied, format_seq};
pub use connect::{connect, ensure_streams};
pub use health::Readiness;
pub use projector::{Projector, Tx};
pub use publisher::{PublishError, catch_up, publish, publish_expecting, subject_head};
pub use snapshot::{SnapshotConfig, Snapshotter};
pub use worker::Worker;
