//! spot-service's tables: the listing itself, whole.
//!
//! One table, so this folder holds one model. It stays a folder for the same reason
//! the others are: which `spot` you mean is the question the path answers, and
//! `domain_models::spot` reading differently from `domain_models::booking` would put
//! that back on the reader.

pub mod spot;

pub use spot::{Spot, SpotPatch};
