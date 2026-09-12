//! Whether this caller may act on this booking, and whether it is still theirs to act
//! on.
//!
//! Both questions are answered from values the caller already holds — the projected
//! row and the id off the verified token — so neither needs a lookup.

use shared::{
    domain_models::booking::Booking,
    error::myerror::{ContextExt, MyResult},
};
use uuid::Uuid;

/// Answered for a booking that does not exist and one the caller does not own —
/// both, deliberately. Whether a booking id exists is not this caller's business.
///
/// Shared with [`crate::service::payment_worker_service`] so one missing booking reads
/// the same way however it was reached, even though that path has no caller to keep it
/// from.
pub const NOT_FOUND: (&str, &str) = ("Not Found", "That booking doesn't exist.");

/// The caller owns this booking and it is in the state they think it is.
pub fn authorize(booking: &Booking, renter_id: &Uuid, expected: &str) -> MyResult<()> {
    // Ownership from the verified token, never from the request. 404 rather than
    // 403: whether a booking id exists isn't this caller's business.
    (booking.renter_id == *renter_id).context_not_found(NOT_FOUND)?;

    // A parameter, not a constant: release leaves `reserved`, cancel leaves
    // `confirmed`.
    (booking.status == expected).context_conflict((
        "Already settled",
        &format!("This booking is already {}.", booking.status),
    ))?;
    Ok(())
}
