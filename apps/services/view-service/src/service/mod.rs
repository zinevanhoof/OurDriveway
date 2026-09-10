//! Holds the pool, and everything a read is besides HTTP.
//!
//! Four services, **one per namespace**, mirroring `route/`. That is the same split the
//! routes already make, and it is the only one that keeps this layer's rule true: the
//! namespace is the authorization, so a service is the set of reads that share a
//! predicate. A method here names its namespace in its name where the repository does
//! (`find_for_host`, `find_for_public`), and there is no shared read for a new one to
//! inherit half of.
//!
//! What lives here rather than in `route/`:
//!
//! - **The pool.** A handler no longer borrows a connection, so it cannot accidentally
//!   take two for reads that must agree — `wallet` and `balance` both depend on that.
//! - **The clock.** `Utc::now()` is read once per request, here, and passed down. The
//!   routes used to each call it; `policy/` takes it as a parameter, which is what keeps
//!   `policy/` testable.
//! - **Value checks that are not HTTP.** The radius ceiling in
//!   [`public_service::PublicService::nearby`] is a rule about what this service will
//!   read, not about how a query string is spelled.
//!
//! What does not: extraction, status codes, and the query structs axum deserializes into.
//!
//! Unlike every other service's, none of these write. There is no `authorize`, no
//! version to answer with and no event to publish — view-service owns no truth. What it
//! shares with them is the shape: handlers call methods, methods hold the handles.

use chrono::{DateTime, Utc};

pub mod account_service;
pub mod host_service;
pub mod public_service;
pub mod renter_service;

/// The instant a booking must have ended before for its payment to count as settled.
///
/// Used by the two money reads — [`account_service::AccountService::wallet`] shows it per
/// row, [`host_service::HostService::balance`] sums either side of it — and shared so the
/// two cannot disagree about where the cutoff is.
///
/// `SETTLEMENT_SECS` is read by **two** services and the two must agree: this decides what
/// a host is shown, and payment-service's copy decides what they can actually withdraw.
/// Different values mean a balance that offers more than the withdraw endpoint will hand
/// over, or less. `k8s/chart/values.yaml` sets both from one entry.
///
/// Not in `policy/` because it reads `CONFIG`, which is the impurity that folder does not
/// have.
pub fn settled_before(now: DateTime<Utc>) -> DateTime<Utc> {
    now - chrono::Duration::seconds(crate::CONFIG.settlement_secs)
}
