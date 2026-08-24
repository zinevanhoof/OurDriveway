//! Whether a requested booking fits: inside the spot's open hours, and clear of
//! what is already taken.
//!
//! Pure — no I/O, no clock. This is the authoritative check, but it is not the
//! guarantee: a concurrent reserve that beat us is caught by the compare-and-swap
//! on publish. This exists to turn that race into a clean 409 naming the offending
//! slot, instead of a retry the renter can't interpret.

use std::collections::HashMap;

use chrono::{Datelike, NaiveDate, Weekday};
use shared::general_models::booking::Booked;
use shared::general_models::spot::{Availability, TimeSlot};

/// "08:30" -> 510. `None` for anything malformed: a slot that won't parse has to
/// fail the booking, never silently become 0 and sail through containment.
fn to_min(hhmm: &str) -> Option<i32> {
    let (h, m) = hhmm.split_once(':')?;
    Some(h.parse::<i32>().ok()? * 60 + m.parse::<i32>().ok()?)
}

/// Half-open `[start, end)`. Zero-length and inverted slots are rejected here.
fn span(slot: &TimeSlot) -> Option<(i32, i32)> {
    let (a, b) = (to_min(&slot.start)?, to_min(&slot.end)?);
    (b > a).then_some((a, b))
}

/// Adjacency is not overlap — 09:00–10:00 and 10:00–11:00 must both be bookable,
/// which is the most common booking pattern there is.
fn overlaps((a1, a2): (i32, i32), (b1, b2): (i32, i32)) -> bool {
    a1 < b2 && b1 < a2
}

fn contains((o1, o2): (i32, i32), (s1, s2): (i32, i32)) -> bool {
    o1 <= s1 && s2 <= o2
}

/// A date's open windows.
///
/// A `single` entry overrides that weekday's recurring hours **even when empty** —
/// an empty array means "closed that day". Same precedence as the frontend's
/// `single?.[iso] ?? weekly?.[weekday]`; diverging here would let the UI offer a
/// slot the server then refuses.
fn open_windows<'a>(availability: &'a Availability, date: &str) -> Option<&'a [TimeSlot]> {
    if let Some(single) = availability.single.get(date) {
        return Some(single);
    }
    let w = &availability.weekly;
    Some(
        match NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?.weekday() {
            Weekday::Mon => &w.monday,
            Weekday::Tue => &w.tuesday,
            Weekday::Wed => &w.wednesday,
            Weekday::Thu => &w.thursday,
            Weekday::Fri => &w.friday,
            Weekday::Sat => &w.saturday,
            Weekday::Sun => &w.sunday,
        },
    )
}

#[derive(Debug, PartialEq)]
pub enum Rejection {
    /// Outside the spot's opening hours for that date.
    Closed { date: String, slot: TimeSlot },
    /// Already held or booked — by someone else, or by this same request twice.
    Taken { date: String, slot: TimeSlot },
    /// Unparseable time, or a request with nothing in it.
    Malformed,
}

/// Authorises a requested booking and returns the **billable minutes**.
///
/// Returning the minutes is deliberate: the caller prices off this number, so the
/// amount charged is derived from exactly the slots that were authorised. A
/// separate pricing pass could drift from the authorised set and charge for
/// something that was refused, or refuse something that was charged.
///
/// `booked` is whatever the caller decided blocks these times
/// (`BookingRepository::taken_for_spot`). Every caller is authorising a booking that
/// does not exist yet, so there is no "skip mine" case here.
pub fn check(
    availability: &Availability,
    booked: &Booked,
    requested: &HashMap<String, Vec<TimeSlot>>,
) -> Result<i64, Rejection> {
    let mut minutes = 0i64;

    for (date, slots) in requested {
        let open: Vec<_> = open_windows(availability, date)
            .ok_or(Rejection::Malformed)?
            .iter()
            .filter_map(span)
            .collect();

        let busy: Vec<_> = booked
            .get(date)
            .map(|s| s.iter().filter_map(span).collect::<Vec<_>>())
            .unwrap_or_default();

        // Slots this request has already claimed. Without this a client asking for
        // 09:00–11:00 and 10:00–12:00 on one date is billed twice for the shared
        // hour and blocks it twice over.
        let mut accepted: Vec<(i32, i32)> = vec![];

        for slot in slots {
            let s = span(slot).ok_or(Rejection::Malformed)?;
            if !open.iter().any(|o| contains(*o, s)) {
                return Err(Rejection::Closed {
                    date: date.clone(),
                    slot: slot.clone(),
                });
            }
            if busy.iter().chain(accepted.iter()).any(|b| overlaps(*b, s)) {
                return Err(Rejection::Taken {
                    date: date.clone(),
                    slot: slot.clone(),
                });
            }
            accepted.push(s);
            minutes += (s.1 - s.0) as i64;
        }
    }

    // An empty request would otherwise authorise a free booking blocking nothing.
    (minutes > 0).then_some(minutes).ok_or(Rejection::Malformed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use shared::general_models::spot::WeeklyAvailability;

    const DATE: &str = "2026-08-03"; // a Monday

    fn slot(start: &str, end: &str) -> TimeSlot {
        TimeSlot {
            start: start.into(),
            end: end.into(),
        }
    }

    fn empty_week() -> WeeklyAvailability {
        WeeklyAvailability {
            monday: vec![],
            tuesday: vec![],
            wednesday: vec![],
            thursday: vec![],
            friday: vec![],
            saturday: vec![],
            sunday: vec![],
        }
    }

    /// Open 09:00–17:00 on Mondays, via the weekly recurrence.
    fn weekly_9_to_17() -> Availability {
        Availability {
            weekly: WeeklyAvailability {
                monday: vec![slot("09:00", "17:00")],
                ..empty_week()
            },
            single: HashMap::new(),
        }
    }

    fn want(start: &str, end: &str) -> HashMap<String, Vec<TimeSlot>> {
        HashMap::from([(DATE.to_string(), vec![slot(start, end)])])
    }

    fn taken(start: &str, end: &str) -> Booked {
        HashMap::from([(DATE.to_string(), vec![slot(start, end)])])
    }

    #[test]
    fn adjacent_slots_do_not_collide() {
        // The regression a naive `a1 <= b2` would cause. Back-to-back hours are the
        // single most common booking pattern, so this is the test that matters most.
        assert!(overlaps((540, 600), (570, 630)));
        assert!(!overlaps((540, 600), (600, 660)));

        let free = check(
            &weekly_9_to_17(),
            &taken("09:00", "10:00"),
            &want("10:00", "11:00"),
        );
        assert_eq!(free, Ok(60));
    }

    #[test]
    fn taken_slots_block() {
        let result = check(
            &weekly_9_to_17(),
            &taken("09:00", "11:00"),
            &want("10:00", "12:00"),
        );
        assert!(matches!(result, Err(Rejection::Taken { .. })));
    }

    #[test]
    fn outside_opening_hours_is_closed() {
        let result = check(&weekly_9_to_17(), &Booked::new(), &want("08:00", "09:00"));
        assert!(matches!(result, Err(Rejection::Closed { .. })));
        // Straddling the closing time is just as closed as starting after it.
        let straddle = check(&weekly_9_to_17(), &Booked::new(), &want("16:00", "18:00"));
        assert!(matches!(straddle, Err(Rejection::Closed { .. })));
    }

    #[test]
    fn an_empty_single_entry_closes_the_day() {
        // `single` overrides `weekly` even when empty — the host blocked that date.
        let availability = Availability {
            weekly: WeeklyAvailability {
                monday: vec![slot("09:00", "17:00")],
                ..empty_week()
            },
            single: HashMap::from([(DATE.to_string(), vec![])]),
        };
        let result = check(&availability, &Booked::new(), &want("10:00", "11:00"));
        assert!(matches!(result, Err(Rejection::Closed { .. })));
    }

    #[test]
    fn request_cannot_overlap_itself() {
        let requested = HashMap::from([(
            DATE.to_string(),
            vec![slot("09:00", "11:00"), slot("10:00", "12:00")],
        )]);
        let result = check(&weekly_9_to_17(), &Booked::new(), &requested);
        assert!(matches!(result, Err(Rejection::Taken { .. })));
    }

    #[test]
    fn minutes_drive_the_price() {
        // 90 minutes at 400 c/h is 600 c. Guards against the check authorising a
        // slot the amount doesn't cover.
        let minutes = check(&weekly_9_to_17(), &Booked::new(), &want("09:00", "10:30")).unwrap();
        assert_eq!(minutes, 90);
        assert_eq!(minutes * 400 / 60, 600);
    }

    #[test]
    fn rejects_empty_malformed_and_inverted() {
        assert_eq!(
            check(&weekly_9_to_17(), &Booked::new(), &HashMap::new()),
            Err(Rejection::Malformed)
        );
        assert_eq!(
            check(&weekly_9_to_17(), &Booked::new(), &want("nope", "11:00")),
            Err(Rejection::Malformed)
        );
        assert_eq!(
            check(&weekly_9_to_17(), &Booked::new(), &want("11:00", "10:00")),
            Err(Rejection::Malformed)
        );
        // Zero-length would otherwise be a free, non-blocking booking.
        assert_eq!(
            check(&weekly_9_to_17(), &Booked::new(), &want("10:00", "10:00")),
            Err(Rejection::Malformed)
        );
    }

    #[test]
    fn weekday_comes_from_the_date_not_the_local_clock() {
        // 2026-08-03 is a Monday and 2026-08-04 a Tuesday, which is closed here.
        let tuesday = HashMap::from([("2026-08-04".to_string(), vec![slot("10:00", "11:00")])]);
        assert!(matches!(
            check(&weekly_9_to_17(), &Booked::new(), &tuesday),
            Err(Rejection::Closed { .. })
        ));
    }
}
