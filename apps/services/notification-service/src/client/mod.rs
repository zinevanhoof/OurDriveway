//! Everything this service says to a third party.
//!
//! One file per outbound integration, and the seam is the point: `service/` decides
//! what should happen, `client/` is the only place that knows whose API says it.
//! Nothing here holds a projection or publishes an event.

pub mod mailer;
