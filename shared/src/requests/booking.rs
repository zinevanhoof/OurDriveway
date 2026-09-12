use std::collections::HashMap;

use garde::Validate;
use serde::Deserialize;
use uuid::Uuid;

use crate::validation::require;

use crate::general_models::spot::{TimeSlot, validate_single};

/// What the booking form posts to reserve slots.
///
/// Note what is **absent**: an amount. The client computes one for display, but the
/// server recomputes it from the spot's price and the minutes it actually
/// authorised — a client-supplied figure would be a price the renter chose.
#[derive(Deserialize, Validate)]
#[serde(rename_all = "camelCase")]
pub struct CreateBookingRequest {
    /// serde rejects a malformed uuid before garde runs, so no length rule is
    /// needed — and nothing downstream has to parse it.
    #[garde(skip)]
    pub spot_id: Uuid,
    /// `"YYYY-MM-DD"` -> slots. Reuses the spot form's date+slot rules verbatim
    /// (`HH:MM`, 30-minute grid, end after start, no overlap or duplicate within a
    /// day, no past dates), because a booking that doesn't fit the grid a spot's
    /// availability is expressed on can never match a window.
    #[garde(custom(validate_single), custom(not_empty), custom(not_in_the_past))]
    pub booked: HashMap<String, Vec<TimeSlot>>,
}

/// A booking with no slots would authorise a free reservation that blocks nothing.
fn not_empty(map: &HashMap<String, Vec<TimeSlot>>, _: &()) -> garde::Result {
    if map.values().any(|slots| !slots.is_empty()) {
        Ok(())
    } else {
        Err(garde::Error::new("Pick at least one time slot."))
    }
}

/// You cannot book a slot that has already happened.
///
/// Here rather than in `validate_single`, for the same reason the spot form keeps
/// its own copy: the answer depends on the clock, so it belongs to the submission
/// and not to the slot map — which is stored on the booking and replayed for years.
fn not_in_the_past(map: &HashMap<String, Vec<TimeSlot>>, _: &()) -> garde::Result {
    let today = chrono::Utc::now().date_naive();
    for key in map.keys() {
        require(
            crate::general_models::spot::parse_date(key)? >= today,
            "Date cannot be in the past",
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot(start: &str, end: &str) -> TimeSlot {
        TimeSlot {
            start: start.into(),
            end: end.into(),
        }
    }

    fn req(date: &str, slots: Vec<TimeSlot>) -> CreateBookingRequest {
        CreateBookingRequest {
            spot_id: Uuid::now_v7(),
            booked: HashMap::from([(date.to_string(), slots)]),
        }
    }

    fn future() -> String {
        (chrono::Utc::now() + chrono::Duration::days(7))
            .date_naive()
            .to_string()
    }

    #[test]
    fn valid_booking_passes() {
        assert!(
            req(&future(), vec![slot("09:00", "11:00")])
                .validate()
                .is_ok()
        );
    }

    #[test]
    fn rejects_empty_and_past_and_off_grid() {
        assert!(req(&future(), vec![]).validate().is_err());
        assert!(
            req("2000-01-01", vec![slot("09:00", "11:00")])
                .validate()
                .is_err()
        );
        assert!(
            req(&future(), vec![slot("09:15", "11:00")])
                .validate()
                .is_err()
        );
    }

    #[test]
    fn rejects_slots_that_overlap_within_a_day() {
        assert!(
            req(
                &future(),
                vec![slot("09:00", "11:00"), slot("10:00", "12:00")]
            )
            .validate()
            .is_err()
        );
    }
}
