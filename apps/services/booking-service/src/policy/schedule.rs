//! Placing a booking's wall-clock times on a real timeline.
//!
//! `booked` is `"YYYY-MM-DD"` plus `"HH:MM"` with no zone attached — it is the
//! *spot's* wall clock, and every deadline in this service is measured against it.
//! Reading those strings as UTC is the bug this whole file exists to prevent: in
//! Brussels it slides an hour-long cutoff by two hours the wrong way in summer.

use chrono::{DateTime, NaiveDateTime, TimeDelta, TimeZone, Utc};
use chrono_tz::Tz;
use shared::{general_models::booking::Booked, general_models::spot::TimeSlot};

/// How long before a booking starts cancelling closes. Any shorter and the host
/// is already standing in the driveway.
const CUTOFF: TimeDelta = TimeDelta::hours(1);

/// Whether a cancel at `now` is still in time.
///
/// `None` when the start can't be determined at all — an unknown zone, or times
/// that don't parse. The caller treats that as "no", because we can't hand out a
/// cancel we can't prove is in time.
///
/// `now` is a parameter rather than a clock read so this stays pure and testable.
pub fn in_time(booked: &Booked, timezone: &str, now: DateTime<Utc>) -> Option<bool> {
    Some(now + CUTOFF <= starts_at(booked, timezone)?)
}

/// The first moment a booking occupies, as a UTC instant.
///
/// The minimum is taken across every date and every slot: `booked` is a `HashMap`,
/// so the first one iterated is not the first one that happens.
fn starts_at(booked: &Booked, timezone: &str) -> Option<DateTime<Utc>> {
    let tz: Tz = timezone.parse().ok()?;
    let first = wall_times(booked, |s| &s.start).min()?;
    instant(first, tz)
}

/// The last moment a booking occupies, as a UTC instant.
///
/// The mirror of `starts_at`, and the field every "is this still to come" filter
/// reads. Same reason for the fold: a `HashMap` of wall-clock strings has no order
/// of its own, so the last date iterated is not the last one that happens.
pub fn ends_at(booked: &Booked, timezone: &str) -> Option<DateTime<Utc>> {
    let tz: Tz = timezone.parse().ok()?;
    let last = wall_times(booked, |s| &s.end).max()?;
    instant(last, tz)
}

/// Every `"YYYY-MM-DD HH:MM"` in `booked`, picking one end of each slot.
fn wall_times<'a>(
    booked: &'a Booked,
    pick: impl Fn(&TimeSlot) -> &String + Copy + 'a,
) -> impl Iterator<Item = NaiveDateTime> + 'a {
    booked
        .iter()
        .flat_map(move |(date, slots)| slots.iter().map(move |s| format!("{date} {}", pick(s))))
        .filter_map(|local| NaiveDateTime::parse_from_str(&local, "%Y-%m-%d %H:%M").ok())
}

/// A wall time in `tz` as an instant.
///
/// A wall time inside a spring-forward gap names no instant at all. The same time an
/// hour later always does; being an hour stricter one night a year beats a booking
/// that can never be cancelled. `earliest` also settles the autumn ambiguity, in the
/// host's favour — for an end that means a booking leaves the Upcoming tab up to an
/// hour early on that one night, which no money depends on.
fn instant(local: NaiveDateTime, tz: Tz) -> Option<DateTime<Utc>> {
    tz.from_local_datetime(&local)
        .earliest()
        .or_else(|| {
            tz.from_local_datetime(&(local + TimeDelta::hours(1)))
                .earliest()
        })
        .map(|dt| dt.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn at(s: &str) -> DateTime<Utc> {
        s.parse().unwrap()
    }

    fn slot(start: &str, end: &str) -> TimeSlot {
        TimeSlot {
            start: start.into(),
            end: end.into(),
        }
    }

    #[test]
    fn cancel_closes_one_hour_before_the_first_slot_in_the_spots_zone() {
        // 09:00 in Brussels on this date is 07:00Z (CEST, UTC+2), so the deadline is
        // 06:00Z. Reading `booked` as bare UTC would put the deadline at 08:00Z and
        // hand out two extra hours of cancelling — the bug this test exists for.
        let booked: Booked = HashMap::from([(
            "2026-08-03".to_string(),
            // Out of order on purpose: `booked` is a HashMap, so "first" has to be a
            // minimum, not whatever happens to iterate first.
            vec![slot("11:00", "12:00"), slot("09:00", "10:00")],
        )])
        .into();
        let tz = "Europe/Brussels";

        assert_eq!(in_time(&booked, tz, at("2026-08-03T05:59:59Z")), Some(true));
        // Exactly on the hour still counts — the boundary is inclusive.
        assert_eq!(in_time(&booked, tz, at("2026-08-03T06:00:00Z")), Some(true));
        assert_eq!(
            in_time(&booked, tz, at("2026-08-03T06:00:01Z")),
            Some(false)
        );
        // Inside the naive-UTC window, and correctly refused anyway.
        assert_eq!(
            in_time(&booked, tz, at("2026-08-03T07:30:00Z")),
            Some(false)
        );
        // An earlier date wins over an earlier clock time on a later date: 22:00 on
        // the 3rd is 20:00Z, so the deadline is 19:00Z — not 07:00 on the 4th.
        let spread: Booked = HashMap::from([
            ("2026-08-04".to_string(), vec![slot("08:00", "09:00")]),
            ("2026-08-03".to_string(), vec![slot("22:00", "23:00")]),
        ])
        .into();
        assert_eq!(in_time(&spread, tz, at("2026-08-03T19:00:00Z")), Some(true));
        assert_eq!(
            in_time(&spread, tz, at("2026-08-03T19:00:01Z")),
            Some(false)
        );
        // Fails closed: an unknown zone can't be proven in time.
        assert_eq!(
            in_time(&booked, "Not/AZone", at("2026-08-03T05:00:00Z")),
            None
        );
    }

    #[test]
    fn ends_at_is_the_last_moment_across_every_day_in_the_spots_zone() {
        // Deliberately out of order, and spanning two days: `booked` is a HashMap, so
        // "last" has to be a maximum. The 4th's 09:00 iterating first must not win
        // over the 3rd's 23:00 — nor the other way round.
        let booked: Booked = HashMap::from([
            (
                "2026-08-04".to_string(),
                vec![slot("08:00", "09:00"), slot("10:00", "11:00")],
            ),
            ("2026-08-03".to_string(), vec![slot("22:00", "23:00")]),
        ])
        .into();

        // 11:00 Brussels on the 4th is 09:00Z in summer. Reading the strings as UTC
        // would answer 11:00Z and keep the booking "upcoming" two hours too long.
        assert_eq!(
            ends_at(&booked, "Europe/Brussels"),
            Some(at("2026-08-04T09:00:00Z"))
        );
        // Same map, other end, so a start/end mix-up can't pass both.
        assert_eq!(
            starts_at(&booked, "Europe/Brussels"),
            Some(at("2026-08-03T20:00:00Z"))
        );
        assert_eq!(ends_at(&booked, "Not/AZone"), None);
        assert_eq!(ends_at(&Booked::new(), "Europe/Brussels"), None);
    }
}
