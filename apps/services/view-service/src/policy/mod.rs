//! Free functions over values: no I/O, no clock, no state.
//!
//! view-service had no `policy/` until the radius query needed one. The bounding box a
//! spatial search scans is arithmetic, and arithmetic that decides which rows a client
//! sees is worth being able to test without a database — which is the tell for whether
//! something belongs here.
//!
//! It exists because PostGIS is unavailable on YSQL. With a geometry type and a GiST
//! index the database would own this entirely; without one, the narrowing is ours.
//!
//! `wallet` joined it for the same kind of reason: which instants a month covers, and
//! which rows count towards which total, are decisions a wallet is wrong about in ways
//! nobody would notice — an off-by-one on a month boundary or a withdrawal counted as
//! spending. Both are arithmetic, so both are testable with nothing running.

//! `page` and `bookings` are the smallest: which window of a list, and — for a booking
//! list — which tab and statuses. A few lines of arithmetic that decide which rows a
//! caller sees, which is the same argument as the other two.

pub mod bookings;
pub mod geo;
pub mod occupancy;
pub mod page;
pub mod rating;
pub mod wallet;
