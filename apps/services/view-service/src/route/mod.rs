//! HTTP shape only: extract, call one service method, pick a status.
//!
//! Ten endpoints in **four namespaces, one predicate each**. The namespace is the
//! authorization, written where the URL is:
//!
//! | namespace | predicate |
//! |---|---|
//! | [`public`] | no relationship required |
//! | [`host`] | `host_id = caller` |
//! | [`renter`] | `renter_id = caller` |
//! | [`account`] | `id = caller` |
//!
//! That replaced a set of names that shared nothing: `/me/spots` and `/spots/{id}/manage`
//! were the same audience under two spellings, `/bookings/{id}` served a renter and a host
//! from one disjunction, and `/spots/{id}` meant "public" only by convention. A route now
//! says who may read it in the same breath as what it returns, and a new one has to pick a
//! namespace before it can be mounted.
//!
//! Three rules shape every handler:
//!
//! - **One route, one service method.** A handler extracts, calls it, and wraps the answer
//!   in `Json`. It never touches a repository, never borrows a connection, and never
//!   branches on who is asking or on which parameters arrived — one namespace's four
//!   predicates are `service/`'s four structs, and the reads that must agree with each
//!   other are inside one method there, on one connection.
//! - **The caller comes from the verified JWT, never from the path or the query string.**
//!   Three of the GraphQL documents these replace passed the reader's own id as a
//!   variable, which was safe only because a table permission clause independently refused
//!   everyone else's rows. The clauses are gone. Extraction is this layer's job, so this
//!   rule stays here: a handler passes `AuthedJwt`'s `user_id` down, and a service method
//!   has no other way to learn who is asking.
//! - **Authorization is the service method you call, not a clause you remember.**
//!   `PublicService::spot` and `HostService::spot` reach different statements over
//!   different columns. A handler appends nothing, and there is no shared rule for a new
//!   read to inherit half of.
//!
//! A row the caller may not see answers 404, not 403, everywhere. A 403 would confirm the
//! existence of something they are not allowed to know exists. The `MyError` that becomes
//! that 404 is raised in `service/`, next to the statement that failed to match — a
//! handler picks no status of its own beyond 200.
//!
//! ## Responses are not projections
//!
//! Every read ends in a `*Response` from `shared::responses::view`, including the ones
//! where it would be field-for-field its projection. A projection changes when a statement
//! needs another column; a response changes when a client needs another field. Those are
//! not the same event, and conflating them is how a column added to a read ends up on the
//! wire.
//!
//! The conversion happens in `service/` rather than here, because for three of these
//! routes it is not a conversion at all: the wallet's totals, its cursor and every row's
//! `pending` are decided by folding what came back, and a handler holding that fold would
//! be a handler holding a rule.

pub mod account;
pub mod host;
pub mod public;
pub mod renter;
