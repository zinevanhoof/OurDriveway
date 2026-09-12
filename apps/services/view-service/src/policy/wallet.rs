//! Which instants a wallet page covers, and the two totals across it.
//!
//! Pure by construction: no I/O, no clock, no state. The month is a parameter and so is
//! `now` wherever one is needed, which is what lets every rule below be tested with no
//! database — the convention in CLAUDE.md.
//!
//! ## Months are UTC
//!
//! A month is a half-open range of instants, and the boundary is midnight UTC. For a
//! renter in Brussels a payment made at 00:30 CEST on the 1st therefore files under the
//! *previous* month. That is visible and it is wrong-ish, but every alternative costs
//! more than it fixes: the viewer's zone would make one host's month disagree with
//! another's over the same booking, and the spot's zone would put one wallet's rows in
//! several months at once.
//!
//! ponytail: UTC boundaries. If this ever matters, the fix is the client sending its
//! offset alongside the month and this function shifting both ends by it — the rest of
//! the query is already parameterised on two instants.

use chrono::{DateTime, Datelike, NaiveDate, Utc};
use shared::projections::wallet::{WalletTransactionProjection, kind};

/// `"2026-08"` — the month an instant falls in, which is also the page it belongs to.
pub fn label(at: DateTime<Utc>) -> String {
    format!("{:04}-{:02}", at.year(), at.month())
}

/// `"2026-08"` to the half-open range `[2026-08-01T00:00Z, 2026-09-01T00:00Z)`.
///
/// `None` for anything that is not a real year and month, which the route turns into a
/// 422 rather than quietly serving some other month's rows.
pub fn bounds(month: &str) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
    let (year, rest) = month.split_once('-')?;
    // Rejects `"2026-8"` and `"2026-008"` before parsing: two digits is the format the
    // labels this compares against are produced in, and accepting a second spelling
    // would make two different strings name one page.
    if rest.len() != 2 || year.len() != 4 {
        return None;
    }

    let year: i32 = year.parse().ok()?;
    let month: u32 = rest.parse().ok()?;

    let start = NaiveDate::from_ymd_opt(year, month, 1)?;
    // December rolls the year. `from_ymd_opt` is what rejects month 0 and 13, so the
    // arithmetic below only ever sees 1..=12.
    let next = if month == 12 {
        NaiveDate::from_ymd_opt(year + 1, 1, 1)?
    } else {
        NaiveDate::from_ymd_opt(year, month + 1, 1)?
    };

    Some((
        start.and_hms_opt(0, 0, 0)?.and_utc(),
        next.and_hms_opt(0, 0, 0)?.and_utc(),
    ))
}

/// `(in, out)` for one month's rows, both as **positive** figures.
///
/// Two rules, and the second is the one worth stating:
///
/// - In is every positive amount: a host's charges, and a renter's refunds.
/// - Out is every negative amount **except payouts**. Withdrawing is moving your own
///   money from one place you own to another; counting it as spending would show a host
///   who withdrew everything they earned a month where they spent as much as they made.
///
/// Folded here rather than as SQL aggregates so the totals are over exactly the rows on
/// screen. Two statements could disagree; one cannot.
pub fn totals(rows: &[WalletTransactionProjection]) -> (i64, i64) {
    let mut money_in = 0;
    let mut money_out = 0;

    for row in rows {
        if row.amount_cents >= 0 {
            money_in += row.amount_cents;
        } else if row.kind != kind::PAYOUT {
            money_out -= row.amount_cents;
        }
    }

    (money_in, money_out)
}

/// Whether a wallet row's money is still ripening.
///
/// **Two sources, and they are not the same question.** A host's charge is pending until
/// the booking behind it has been over for `SETTLEMENT_SECS` — so the row carries
/// `settles_at`, the booking's end, and this compares it. A withdrawal is pending while
/// the transfer is in flight, which the row already knows as `pending_now` because it is
/// a status rather than a deadline.
///
/// `settles_at` is `None` for every row that cannot ripen: a renter's charge, either side
/// of a refund, and any payout. Money that already moved is not waiting for anything.
///
/// This used to be `(p.status = 'succeeded' AND b.status = 'confirmed' AND b.ends_at >= $4)`
/// inside the wallet's `SELECT`, where nothing could test it and the settlement cutoff had
/// to be bound into a query that otherwise has no use for it. The statement now returns
/// the booking's end and this decides what it means.
pub fn pending(
    settles_at: Option<DateTime<Utc>>,
    pending_now: bool,
    settled_before: DateTime<Utc>,
) -> bool {
    pending_now || settles_at.is_some_and(|ends_at| ends_at >= settled_before)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn row(kind: &str, amount_cents: i64) -> WalletTransactionProjection {
        WalletTransactionProjection {
            id: format!("{kind}:{amount_cents}"),
            kind: kind.to_string(),
            amount_cents,
            occurred_at: Utc.with_ymd_and_hms(2026, 8, 14, 9, 0, 0).unwrap(),
            settles_at: None,
            pending_now: false,
            title: None,
            booked: None,
            timezone: None,
        }
    }

    #[test]
    fn a_month_is_half_open_from_midnight_utc() {
        let (start, end) = bounds("2026-08").unwrap();
        assert_eq!(start, Utc.with_ymd_and_hms(2026, 8, 1, 0, 0, 0).unwrap());
        assert_eq!(end, Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0).unwrap());
    }

    /// The one arithmetic edge in `bounds`: the end of December is January of the next
    /// year, not month 13.
    #[test]
    fn december_rolls_into_the_next_year() {
        let (start, end) = bounds("2026-12").unwrap();
        assert_eq!(start, Utc.with_ymd_and_hms(2026, 12, 1, 0, 0, 0).unwrap());
        assert_eq!(end, Utc.with_ymd_and_hms(2027, 1, 1, 0, 0, 0).unwrap());
    }

    #[test]
    fn february_is_whatever_the_calendar_says() {
        // A leap year, so the range is 29 days rather than 28 — which is only correct
        // because the end is the *next month's* first day rather than a day count.
        let (start, end) = bounds("2028-02").unwrap();
        assert_eq!((end - start).num_days(), 29);
    }

    #[test]
    fn a_month_that_is_not_a_month_is_refused() {
        for bad in [
            "",
            "2026",
            "2026-",
            "2026-00",
            "2026-13",
            "2026-8",
            "2026-008",
            "20260-8",
            "abcd-ef",
            "2026-08-14",
        ] {
            assert!(bounds(bad).is_none(), "{bad} should not parse");
        }
    }

    #[test]
    fn a_label_round_trips_through_bounds() {
        let at = Utc.with_ymd_and_hms(2026, 1, 31, 23, 59, 59).unwrap();
        let (start, end) = bounds(&label(at)).unwrap();
        assert!(start <= at && at < end);
    }

    #[test]
    fn in_is_positive_and_out_is_the_magnitude_of_the_negatives() {
        let (money_in, money_out) = totals(&[
            row(kind::IN, 2_500),
            row(kind::OUT, -1_000),
            row(kind::REFUND, 400),
        ]);
        assert_eq!(money_in, 2_900);
        assert_eq!(money_out, 1_000);
    }

    /// The rule the placeholder component already commented and the server now owns:
    /// withdrawing is not spending.
    #[test]
    fn a_payout_counts_towards_neither_total() {
        let (money_in, money_out) = totals(&[row(kind::IN, 5_000), row(kind::PAYOUT, -5_000)]);
        assert_eq!(money_in, 5_000);
        assert_eq!(money_out, 0);
    }

    /// A host whose booking was cancelled: the charge came in and went back out, and
    /// the month nets to zero without either figure being hidden.
    #[test]
    fn a_hosts_refund_is_money_out() {
        let (money_in, money_out) = totals(&[row(kind::IN, 1_200), row(kind::REFUND, -1_200)]);
        assert_eq!(money_in, 1_200);
        assert_eq!(money_out, 1_200);
    }

    #[test]
    fn an_empty_month_is_two_zeroes() {
        assert_eq!(totals(&[]), (0, 0));
    }

    /// The cutoff for every case below: a booking that ended before this has settled.
    fn cutoff() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, 14, 0, 0, 0).unwrap()
    }

    #[test]
    fn a_charge_on_a_booking_that_has_not_settled_is_pending() {
        let ends_after = Utc.with_ymd_and_hms(2026, 8, 20, 0, 0, 0).unwrap();
        assert!(pending(Some(ends_after), false, cutoff()));
    }

    #[test]
    fn a_charge_on_a_booking_that_settled_is_not() {
        let ends_before = Utc.with_ymd_and_hms(2026, 8, 1, 0, 0, 0).unwrap();
        assert!(!pending(Some(ends_before), false, cutoff()));
    }

    /// The boundary is the cutoff itself, and it is inclusive on the pending side —
    /// matching `b.ends_at >= $4`, which is what the SQL said.
    #[test]
    fn the_cutoff_instant_itself_is_still_pending() {
        assert!(pending(Some(cutoff()), false, cutoff()));
    }

    /// A withdrawal in flight ripens on a status, not a deadline, so it carries no
    /// `settles_at` at all and must still read as pending.
    #[test]
    fn a_withdrawal_in_flight_is_pending_with_no_settles_at() {
        assert!(pending(None, true, cutoff()));
    }

    /// Everything that already moved: a renter's charge, either side of a refund, a
    /// completed payout. Nothing is waiting, so nothing is pending.
    #[test]
    fn a_row_that_can_never_ripen_is_never_pending() {
        assert!(!pending(None, false, cutoff()));
    }
}
