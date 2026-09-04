//! The combined read model: the one database a browser can reach.
//!
//! Five tables over the four streams view-service consumes — PAYMENTS contributes
//! two, `payment` and `payout` — and every one of them a *different* table from the
//! same-named one in the owning service's private database, hence the `View` prefix
//! throughout. `ViewSpot` is what a map query returns; `domain_models::spot::Spot` is
//! what spot-service stores.
//!
//! **None of these models carry their `record<>` link columns.** `spot.owner`,
//! `booking.spot`, `booking.renter` and `payout.owner` are set by a subquery that
//! resolves to NONE when the referenced row has not been projected yet, and are
//! backfilled when it arrives — streams have no cross-stream ordering, so a spot
//! routinely lands before its owner. Writing a pointer unconditionally would be a
//! different behaviour, so the links are left to the repositories' own statements
//! and deliberately out of the row shape — and, now that the patch structs are
//! hand-written, out of those too, which makes it a compile error rather than a
//! convention.
//!
//! The consequence is a rule: on the tables that have one, **never `upsert`**.
//! `CONTENT` replaces the whole record body and would erase both the link and any
//! column another stream owns. Use `merge`, or `upsert` followed by relinking in the
//! same transaction — each model says which.

pub mod booking;
pub mod payment;
pub mod payout;
pub mod spot;
pub mod user;

pub use booking::{ViewBooking, ViewBookingPatch};
pub use payment::{ViewPayment, ViewPaymentPatch};
pub use payout::ViewPayout;
pub use spot::{ViewSpot, ViewSpotPatch};
pub use user::{ViewUser, ViewUserPatch};
