//! Every table in the system, grouped by the service that owns the database it
//! lives in.
//!
//! One folder per service, and the folder is what disambiguates: there are four
//! different tables called `spot` and three called `booking`, each in a private
//! database with its own columns. `booking::spot` is the handful of columns
//! booking-service mirrors to price a reservation; `spot::spot` is the whole
//! listing; `view::spot` is what a map query returns. They are not interchangeable
//! and no two of them can be reached from one connection.
//!
//! Type names stay prefixed (`SpotMirror`, `BookingMirror`, `ViewSpot`) rather
//! than relying on the module path alone, so a `use` at the top of a file is enough
//! to tell which one is in play without reading it back.
//!
//! Each folder re-exports its models flat, so callers write
//! `domain_models::payment::Payment` and never `payment::payment::Payment`.

pub mod booking;
pub mod payment;
pub mod spot;
pub mod user;
pub mod view;
