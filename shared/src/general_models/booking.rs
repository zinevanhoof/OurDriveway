use std::collections::HashMap;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use surrealdb::types::SurrealValue;

use crate::general_models::spot::TimeSlot;

/// Slots that are taken on a spot: `"YYYY-MM-DD"` -> slots.
///
/// Denormalized onto the spot record so availability is one point read rather than
/// an index probe plus a fetch per booking. Deliberately the same shape as a spot's
/// `availability.single`, so every reader — including the frontend — consumes it
/// with no reshaping.
///
/// It carries **no** hold expiry and no booking id. Everything in here is taken,
/// full stop; a reader never has to filter it. A hold that lapses is removed by the
/// expiry sweeper publishing `Released`, not by readers learning to skip it.
pub type Booked = HashMap<String, Vec<TimeSlot>>;

/// The projected booking fields the fold needs. Both services select exactly these.
#[derive(Clone, Debug, Serialize, Deserialize, SurrealValue)]
pub struct BookingRow {
    pub status: String,
    pub booked: Booked,
}

/// Statuses whose slots are free again. Anything else blocks — including a status
/// this build has never heard of, because the safe direction for availability is to
/// keep a slot unavailable rather than sell it twice.
const RELEASED: [&str; 2] = ["released", "cancelled"];

/// Rebuilds a spot's `booked` map from its projected booking rows.
///
/// A full recompute rather than an incremental merge, which is what makes applying
/// a BOOKINGS event idempotent: re-delivering the same message twice produces the
/// same map, where appending slots twice would not.
///
/// `at` comes from the event envelope and is used **only** to prune past dates.
/// Nothing here reads a clock — two replicas replaying the same log must land on
/// byte-identical maps, which is also why the slots are sorted.
pub fn fold_booked(rows: &[BookingRow], at: DateTime<Utc>) -> Booked {
    // A day of slack so a booking still running locally in the spot's timezone
    // isn't pruned by a UTC date rollover.
    let cutoff = (at - Duration::days(1)).date_naive().to_string();

    let mut booked: Booked = HashMap::new();
    for row in rows {
        if RELEASED.contains(&row.status.as_str()) {
            continue;
        }
        for (date, slots) in &row.booked {
            if *date < cutoff {
                continue;
            }
            booked
                .entry(date.clone())
                .or_default()
                .extend(slots.clone());
        }
    }
    for slots in booked.values_mut() {
        slots.sort_by(|a, b| (&a.start, &a.end).cmp(&(&b.start, &b.end)));
    }
    booked
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(status: &str, date: &str, start: &str, end: &str) -> BookingRow {
        BookingRow {
            status: status.into(),
            booked: HashMap::from([(
                date.to_string(),
                vec![TimeSlot {
                    start: start.into(),
                    end: end.into(),
                }],
            )]),
        }
    }

    fn at() -> DateTime<Utc> {
        "2026-08-01T12:00:00Z".parse().unwrap()
    }

    #[test]
    fn holds_and_confirmed_block_released_does_not() {
        let rows = [
            row("reserved", "2026-08-03", "09:00", "10:00"),
            row("confirmed", "2026-08-03", "11:00", "12:00"),
            row("released", "2026-08-03", "14:00", "15:00"),
        ];
        let booked = fold_booked(&rows, at());
        assert_eq!(
            booked["2026-08-03"]
                .iter()
                .map(|s| s.start.as_str())
                .collect::<Vec<_>>(),
            vec!["09:00", "11:00"],
        );
    }

    #[test]
    fn an_unknown_status_blocks() {
        // Fail closed: better to keep a slot unavailable than to sell it twice
        // because a future status wasn't in the exclude list.
        let booked = fold_booked(
            &[row("some_future_status", "2026-08-03", "09:00", "10:00")],
            at(),
        );
        assert_eq!(booked["2026-08-03"].len(), 1);
    }

    #[test]
    fn prunes_past_dates_but_keeps_the_far_future() {
        // No horizon cap: a booking years out is stored like any other. Only the
        // past falls off, which is what bounds the record.
        let rows = [
            row("confirmed", "2020-01-01", "09:00", "10:00"),
            row("confirmed", "2029-12-31", "09:00", "10:00"),
        ];
        let booked = fold_booked(&rows, at());
        assert!(!booked.contains_key("2020-01-01"));
        assert!(booked.contains_key("2029-12-31"));
    }

    #[test]
    fn yesterday_survives_the_days_slack() {
        // The spot's local day can still be in progress when UTC has rolled over.
        let booked = fold_booked(&[row("confirmed", "2026-07-31", "23:00", "23:30")], at());
        assert!(booked.contains_key("2026-07-31"));
    }

    #[test]
    fn output_is_ordered_so_replicas_agree() {
        // HashMap iteration order varies per process; without the sort two replicas
        // would store the same slots in different array orders and never converge.
        let rows = [
            row("confirmed", "2026-08-03", "14:00", "15:00"),
            row("confirmed", "2026-08-03", "09:00", "10:00"),
        ];
        let a = fold_booked(&rows, at());
        let mut reversed = rows.clone();
        reversed.reverse();
        assert_eq!(
            a["2026-08-03"]
                .iter()
                .map(|s| s.start.clone())
                .collect::<Vec<_>>(),
            fold_booked(&reversed, at())["2026-08-03"]
                .iter()
                .map(|s| s.start.clone())
                .collect::<Vec<_>>(),
        );
    }
}
