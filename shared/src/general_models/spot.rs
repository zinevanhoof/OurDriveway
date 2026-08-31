use std::collections::HashMap;

use chrono::NaiveDate;
use garde::Validate;
// Serialize is here because these types travel inside events on the wire, not
// just into the database.
use serde::{Deserialize, Serialize};

use crate::validation::require;

// These four are stored as `jsonb` columns rather than as flattened scalars and
// nested arrays, so they need no database-specific derive at all — `Serialize` and
// `Deserialize` are the whole contract, and the field carrying them is marked
// `#[sqlx(json)]` on the owning model.
//
// That is a real simplification over what it replaced: the SurrealDB schema spelled
// out every leaf, down to `availability.weekly.*.*.start`, because SCHEMAFULL demanded
// it. Nothing in any query reaches into these — they are read and written whole — so
// the leaves were declaration without leverage.

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Address {
    pub line1: String,
    pub line2: Option<String>,
    pub city: String,
    pub postal_code: String,
    pub region: Option<String>,
    pub country: String,
    pub formatted: String,
}

/// The availability grid, and the rules it is held to.
///
/// **The rules live on the stored type, and that is safe here specifically because
/// `garde::Validate` is not `Deserialize`.** Validation runs in exactly one place —
/// `shared::extract::Valid` — so a projector decoding this out of a SPOTS event
/// never runs it. Putting the same rules in a `Deserialize` impl would run them on
/// every replay and reject history that was legal when it was written.
///
/// What that buys: there is no `AvailabilityRequest` any more. The request form and
/// the stored form were field-for-field identical plus a `From` impl.
///
/// Only **timeless** rules belong here — shape, ordering, overlap. Anything that
/// depends on *when* it is being asked stays on the request field, because a stored
/// grid may legitimately violate it: see `no_past_dates` in
/// `requests::spot::availability`, which a spot listed last March fails and should.
#[derive(Clone, Debug, Serialize, Deserialize, Validate)]
pub struct Availability {
    #[garde(dive)]
    pub weekly: WeeklyAvailability,
    #[garde(custom(validate_single))]
    pub single: HashMap<String, Vec<TimeSlot>>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Validate)]
pub struct WeeklyAvailability {
    #[garde(custom(validate_slots))]
    pub monday: Vec<TimeSlot>,
    #[garde(custom(validate_slots))]
    pub tuesday: Vec<TimeSlot>,
    #[garde(custom(validate_slots))]
    pub wednesday: Vec<TimeSlot>,
    #[garde(custom(validate_slots))]
    pub thursday: Vec<TimeSlot>,
    #[garde(custom(validate_slots))]
    pub friday: Vec<TimeSlot>,
    #[garde(custom(validate_slots))]
    pub saturday: Vec<TimeSlot>,
    #[garde(custom(validate_slots))]
    pub sunday: Vec<TimeSlot>,
}

/// Slots are judged collectively — a slot is only wrong *relative to the others on
/// its day* — so there are no per-field rules and no `Validate` derive. See
/// [`validate_slots`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeSlot {
    pub start: String, // "08:00"
    pub end: String,   // "18:00"
}

/// Full mirror of the frontend weekday-slot rules: `HH:MM`, 30-minute increments,
/// end after start, and no overlaps or duplicates within the day.
///
/// `pub` because a booking request holds its slots to the identical rules — see
/// `requests::booking`. Two copies of these would drift.
pub fn validate_slots(slots: &Vec<TimeSlot>, _: &()) -> garde::Result {
    for slot in slots {
        check_time(&slot.start)?;
        check_time(&slot.end)?;
        // String compare is correct for zero-padded HH:MM.
        require(
            slot.start < slot.end,
            "End time must be after the start time.",
        )?;
    }
    for (i, a) in slots.iter().enumerate() {
        for b in &slots[i + 1..] {
            require(
                !(a.start == b.start && a.end == b.end),
                "This time slot already exists.",
            )?;
            require(
                !(a.start < b.end && a.end > b.start),
                "This time slot overlaps with an existing slot.",
            )?;
        }
    }
    Ok(())
}

/// One-off dates: the key has to be a real `YYYY-MM-DD`, and each day's slots
/// follow the same rules as the weekly ones.
///
/// Deliberately **not** the past-date check, which used to live here. That one is
/// about the moment of submission, not about the grid, and a stored grid whose
/// dates have since passed is perfectly valid data — see the type doc.
pub fn validate_single(map: &HashMap<String, Vec<TimeSlot>>, _: &()) -> garde::Result {
    for (key, slots) in map {
        parse_date(key)?;
        validate_slots(slots, &())?;
    }
    Ok(())
}

/// `YYYY-MM-DD`, as a date that actually exists.
pub fn parse_date(key: &str) -> Result<NaiveDate, garde::Error> {
    NaiveDate::parse_from_str(key, "%Y-%m-%d").map_err(|_| garde::Error::new("Invalid date"))
}

/// `HH:MM`, 00–23 hours, 00–59 minutes, minutes a multiple of 30.
fn check_time(value: &str) -> garde::Result {
    let (h, m) = value
        .split_once(':')
        .ok_or_else(|| garde::Error::new("Invalid time"))?;
    let hour: u32 = h.parse().map_err(|_| garde::Error::new("Invalid time"))?;
    let min: u32 = m.parse().map_err(|_| garde::Error::new("Invalid time"))?;
    require(
        h.len() == 2 && m.len() == 2 && hour < 24 && min < 60 && min % 30 == 0,
        "Time must be in 30 minute increments.",
    )
}
