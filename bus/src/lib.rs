//! NATS plumbing shared by every service: connecting, declaring streams, and
//! reporting whether this instance's projections are current enough to serve.
//!
//! Separate from `shared` so that crate stays runtime-free — see bus/Cargo.toml.

pub mod connect;
pub mod health;
pub mod projector;
pub mod publisher;
pub mod snapshot;

pub use connect::{connect, ensure_streams};
pub use health::Readiness;
pub use projector::Projector;
pub use publisher::{PublishError, publish, publish_expecting, subject_head};
pub use snapshot::{SnapshotConfig, Snapshotter};
