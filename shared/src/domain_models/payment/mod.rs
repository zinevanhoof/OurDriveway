//! payment-service's tables: what was charged, what was withdrawn, and the slice of
//! a booking needed to decide either.
//!
//! The most private database in the system — it holds Stripe intent ids and what
//! every renter paid, and every table in it is additionally `PERMISSIONS NONE`.
//!
//! [`booking::BookingMirror`] is a **mirror** of the BOOKINGS stream, and a
//! different table from [`super::booking::Booking`]: note the column is
//! `amount_cents` here and `amount` there.

pub mod booking;
pub mod payment;
pub mod payout;

pub use booking::{BookingMirror, BookingMirrorPatch};
pub use payment::{Earnings, Payment, PaymentPatch, status};
pub use payout::{Payout, PayoutPatch};
