//! Request/reply contracts between services.
//!
//! The third way data crosses a service boundary here, and the narrowest. The other two
//! are [`crate::events`] — a stream you consume because you *react* to it — and the
//! projections built from them. This one is for a value you need at a single moment,
//! keyed by an id you already hold, from a context you have no other reason to follow.
//!
//! The rule that decides between them: **a point read by key is a lookup, a query across
//! the data is a projection.** payment-service joins its payments against every confirmed
//! booking of a host to compute earnings, so it projects bookings. It needs one spot's
//! title to label a checkout, so it asks.
//!
//! Everything here is best-effort by construction. A request couples the caller's
//! availability to the responder's, which is exactly what the event log was chosen to
//! avoid, so nothing load-bearing may travel this way — no amounts, no authorization, no
//! state a decision turns on. If the answer not arriving would break something, it
//! belongs in an event.

pub mod spot;
