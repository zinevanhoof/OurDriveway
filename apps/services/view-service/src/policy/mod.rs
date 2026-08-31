//! Free functions over values: no I/O, no clock, no state.
//!
//! view-service had no `policy/` until the radius query needed one. The bounding box a
//! spatial search scans is arithmetic, and arithmetic that decides which rows a client
//! sees is worth being able to test without a database — which is the tell for whether
//! something belongs here.
//!
//! It exists because PostGIS is unavailable on YSQL. With a geometry type and a GiST
//! index the database would own this entirely; without one, the narrowing is ours.

pub mod geo;
