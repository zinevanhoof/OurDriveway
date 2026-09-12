//! What the listing form is held to *beyond* what the grid itself guarantees.
//!
//! There is no `AvailabilityRequest` any more. The shape rules — `HH:MM`, the
//! 30-minute grid, ordering, overlaps — moved onto
//! [`crate::general_models::spot::Availability`], which is safe because
//! `garde::Validate` only ever runs where it is called (`shared::extract::Valid`)
//! and never on the event path the way a `Deserialize` impl would.
//!
//! What is left here is the two rules that are about the *submission* rather than
//! the grid, and so must not live on the stored type: a listing needs at least one
//! slot, and a newly submitted one-off date cannot be in the past. A grid projected
//! out of a year-old event legitimately fails the second one.

use chrono::Utc;

use crate::general_models::spot::{Availability, parse_date};
use crate::validation::require;

/// A spot with no slots at all can never be booked, so at least one weekly or
/// one-off slot is required. Mirrors the same guard in AddSpotView.
///
/// `pub(super)` rather than private: it is a rule about this type, but it is
/// applied on the *field* in create and update, which live one module over.
pub(super) fn has_any_slot(value: &Availability, _: &()) -> garde::Result {
    let w = &value.weekly;
    let weekly = [
        &w.monday,
        &w.tuesday,
        &w.wednesday,
        &w.thursday,
        &w.friday,
        &w.saturday,
        &w.sunday,
    ]
    .iter()
    .any(|slots| !slots.is_empty());

    require(
        weekly || value.single.values().any(|slots| !slots.is_empty()),
        "Add at least one availability slot.",
    )
}

/// One-off dates being submitted now have to be today or later.
///
/// Split out of the grid's own rules on purpose: this is the one availability rule
/// whose answer changes with the clock, so it belongs to the act of submitting, not
/// to the value. Leaving it on `Availability` would have the type assert something
/// its own stored instances stop satisfying the day after they are written.
///
/// `pub(super)` for the same reason as [`has_any_slot`] — applied on the field.
pub(super) fn no_past_dates(value: &Availability, _: &()) -> garde::Result {
    // ponytail: past-date compared against UTC today, not the spot's timezone
    // (unknown until after geocoding). Fine ±1 day at the boundary; make it
    // tz-aware if the zone is resolved earlier.
    let today = Utc::now().date_naive();
    for key in value.single.keys() {
        require(parse_date(key)? >= today, "Date cannot be in the past")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::{create, slot};
    use garde::Validate;

    #[test]
    fn rejects_availability_without_any_slot() {
        assert!(create(500, vec![]).validate().is_err());
    }

    #[test]
    fn accepts_availability_with_only_a_single_date() {
        let mut r = create(500, vec![]);
        let tomorrow = (chrono::Utc::now() + chrono::Duration::days(1)).date_naive();
        r.availability
            .single
            .insert(tomorrow.to_string(), vec![slot("08:00", "10:00")]);
        assert!(r.validate().is_ok());
    }

    #[test]
    fn rejects_past_single_date() {
        let mut r = create(500, vec![]);
        r.availability
            .single
            .insert("2000-01-01".into(), vec![slot("08:00", "10:00")]);
        assert!(r.validate().is_err());
    }

    /// The shape rules now live on `Availability` itself, so this is also the test
    /// that they are still reachable through `dive` from the request.
    #[test]
    fn rejects_malformed_time() {
        // not a 30-minute increment
        assert!(
            create(500, vec![slot("08:15", "10:00")])
                .validate()
                .is_err()
        );
        // out of range
        assert!(
            create(500, vec![slot("25:00", "26:00")])
                .validate()
                .is_err()
        );
    }

    #[test]
    fn rejects_overlapping_slots() {
        assert!(
            create(500, vec![slot("08:00", "10:00"), slot("09:00", "11:00")])
                .validate()
                .is_err()
        );
    }
}
