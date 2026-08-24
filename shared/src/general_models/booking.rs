use std::collections::HashMap;

use crate::general_models::spot::TimeSlot;

/// The slots a booking occupies: `"YYYY-MM-DD"` -> slots.
///
/// Bare wall-clock strings in the *spot's* timezone, with no zone attached and no
/// hold expiry — `ends_at` is the folded instant, and `hold_until` lives on the
/// booking row.
///
/// Deliberately the same shape as a spot's `availability.single`, so every reader —
/// including the frontend — subtracts one from the other with no reshaping.
pub type Booked = HashMap<String, Vec<TimeSlot>>;
