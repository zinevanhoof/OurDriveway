//! HTTP shape only: extract, call one repository function, pick a status.
//!
//! Eight endpoints, and **each one has a single responsibility**. Three rules shape all
//! of them:
//!
//! - **One route, one audience, one projection, one repository call.** A handler never
//!   branches on who is asking or on which parameters arrived. `/spots/:id` is the
//!   public shape and `/spots/:id/manage` is the host's; `/me/spots` is the caller's own
//!   listings and `/spots/nearby` is the map. Each of those was previously a mode of
//!   some other route, selected by a query parameter or cut out of a wider response
//!   afterwards.
//! - **The caller comes from the verified JWT, never from the query string.** Three of
//!   the GraphQL documents these replace passed the reader's own id as a variable, which
//!   was safe only because a table permission clause independently refused everyone
//!   else's rows. The clauses are gone, so the caller's own reads live under `/me`.
//! - **Authorization is the repository function you call, not a clause you remember.**
//!   `find_public_by_id` and `find_owner_by_id` are different statements over different
//!   columns. A handler appends nothing, and there is no shared rule for a new read to
//!   inherit half of.
//!
//! A row the caller may not see answers 404, not 403, everywhere. A 403 would confirm
//! the existence of something they are not allowed to know exists.

pub mod booking;
pub mod me;
pub mod spot;
