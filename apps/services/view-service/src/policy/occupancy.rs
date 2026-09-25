//! Whether a booking is happening at an instant.
//!
//! `booked` is `"YYYY-MM-DD"` plus `"HH:MM"` in the **spot's** wall clock, with no zone
//! attached, so "now" has to be read on that clock before it can be compared. Reading the
//! strings as UTC would call a Brussels booking active two hours late in summer.

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use shared::general_models::booking::Booked;

/// Whether one of `booked`'s slots covers `now`, on `timezone`'s wall clock.
///
/// Start inclusive, end exclusive — a 09:00–10:00 slot is over at 10:00, when the next
/// one may begin. `false` for a zone that does not parse: a spot whose clock cannot be
/// read is not claimed to be occupied.
pub fn active_now(booked: &Booked, timezone: &str, now: DateTime<Utc>) -> bool {
    let Ok(tz) = timezone.parse::<Tz>() else {
        return false;
    };
    let there = now.with_timezone(&tz);
    let (date, time) = (
        there.format("%Y-%m-%d").to_string(),
        there.format("%H:%M").to_string(),
    );

    booked
        .get(&date)
        .is_some_and(|slots| slots.iter().any(|s| s.start <= time && time < s.end))
}

#[cfg(test)]
mod tests {
    use super::*;
    use shared::general_models::spot::TimeSlot;
    use std::collections::HashMap;

    fn booked(date: &str, start: &str, end: &str) -> Booked {
        HashMap::from([(
            date.to_string(),
            vec![TimeSlot {
                start: start.into(),
                end: end.into(),
            }],
        )])
        .into()
    }

    fn at(s: &str) -> DateTime<Utc> {
        s.parse().unwrap()
    }

    /// 09:30 in Brussels on this date is 07:30Z (CEST, UTC+2). Read as UTC, the slot
    /// would not have started yet — the bug this exists for.
    #[test]
    fn reads_now_on_the_spots_clock() {
        let b = booked("2026-08-03", "09:00", "10:00");
        assert!(active_now(&b, "Europe/Brussels", at("2026-08-03T07:30:00Z")));
        assert!(!active_now(&b, "UTC", at("2026-08-03T07:30:00Z")));
    }

    #[test]
    fn start_is_inclusive_and_end_is_not() {
        let b = booked("2026-08-03", "09:00", "10:00");
        assert!(active_now(&b, "UTC", at("2026-08-03T09:00:00Z")));
        assert!(!active_now(&b, "UTC", at("2026-08-03T10:00:00Z")));
    }

    #[test]
    fn another_day_or_an_unknown_zone_is_not_active() {
        let b = booked("2026-08-03", "09:00", "10:00");
        assert!(!active_now(&b, "UTC", at("2026-08-04T09:30:00Z")));
        assert!(!active_now(&b, "Not/AZone", at("2026-08-03T09:30:00Z")));
    }
}
