//! booking-service's tables: its own bookings, plus the slice of a spot it needs to
//! authorize and price one.
//!
//! [`spot::SpotMirror`] is a **mirror**, fed by the SPOTS stream and carrying two
//! columns no SPOTS event knows about — the folded slot map and the per-spot
//! compare-and-swap cursor. It is a different table from [`super::spot::Spot`], in a
//! different database, sharing only a name.

pub mod booking;
pub mod spot;

pub use booking::{Booking, status};
pub use spot::{SpotMirror, SpotMirrorPatch};
