//! Which bookings a slice of the host's list holds: the tab, the statuses, the window,
//! and what comes next.
//!
//! Parsing over four query parameters, which is exactly the kind of thing that is wrong
//! in ways nobody notices — a window that starts one row late, a "next offset" that
//! points past the end and renders empty, an offset that goes negative and takes the
//! statement with it. Here rather than in the service so all of it is asserted below
//! with nothing running.
//!
//! Every parser answers `None` for a request that is not a slice of this list, and the
//! service turns any `None` into one 422. Nothing is clamped: a limit of 500 means the
//! caller wants something this endpoint does not serve, and silently serving 50 would
//! hide that.

/// Rows when the caller does not say. What the paged screen reads.
pub const DEFAULT_LIMIT: i64 = 20;
/// The most one request may ask for.
pub const MAX_LIMIT: i64 = 50;

/// Every status a booking row can hold, and so what an absent `status` means.
pub const STATUSES: [&str; 4] = ["reserved", "confirmed", "cancelled", "released"];

/// `upcoming` (or absent) is `false`, `past` is `true` — the flag the repository takes.
/// What is over reads most recent first, what is still to come reads soonest first.
pub fn scope(scope: Option<&str>) -> Option<bool> {
    match scope.unwrap_or("upcoming") {
        "upcoming" => Some(false),
        "past" => Some(true),
        _ => None,
    }
}

/// A comma-separated `status`, e.g. `confirmed,reserved`. Absent is every status.
///
/// Returns the `'static` spellings from [`STATUSES`] rather than slices of the query,
/// so what reaches the statement is only ever one of four known strings.
pub fn statuses(status: Option<&str>) -> Option<Vec<&'static str>> {
    let Some(status) = status else {
        return Some(STATUSES.to_vec());
    };

    status
        .split(',')
        .map(|s| STATUSES.iter().copied().find(|known| *known == s.trim()))
        .collect::<Option<Vec<_>>>()
        .filter(|list| !list.is_empty())
}

/// `1..=MAX_LIMIT`, [`DEFAULT_LIMIT`] when absent.
pub fn limit(limit: Option<i64>) -> Option<i64> {
    let limit = limit.unwrap_or(DEFAULT_LIMIT);
    (1..=MAX_LIMIT).contains(&limit).then_some(limit)
}

/// `0` when absent. Negative is the caller computing offsets itself and getting it wrong.
pub fn offset(offset: Option<i64>) -> Option<i64> {
    let offset = offset.unwrap_or(0);
    (offset >= 0).then_some(offset)
}

/// The offset to ask for after this window, or `None` at the end of the list.
///
/// Computed from the total rather than from "did this window come back full", which is
/// the version that hands out one empty window at every exact multiple of the limit.
pub fn next_offset(offset: i64, limit: i64, total: i64) -> Option<i64> {
    (offset + limit < total).then_some(offset + limit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_parameters_are_the_first_window_of_every_upcoming_booking() {
        assert_eq!(scope(None), Some(false));
        assert_eq!(statuses(None), Some(STATUSES.to_vec()));
        assert_eq!(limit(None), Some(DEFAULT_LIMIT));
        assert_eq!(offset(None), Some(0));
    }

    #[test]
    fn parses_what_the_screens_send() {
        assert_eq!(scope(Some("past")), Some(true));
        assert_eq!(statuses(Some("confirmed")), Some(vec!["confirmed"]));
        assert_eq!(
            statuses(Some("cancelled,released")),
            Some(vec!["cancelled", "released"])
        );
        assert_eq!(limit(Some(2)), Some(2));
        assert_eq!(limit(Some(MAX_LIMIT)), Some(MAX_LIMIT));
        assert_eq!(offset(Some(40)), Some(40));
    }

    /// Each of these is the caller asking for something that is not a slice of this list.
    #[test]
    fn rejects_rather_than_clamps() {
        assert_eq!(scope(Some("cancelled")), None);
        assert_eq!(statuses(Some("paid")), None);
        assert_eq!(statuses(Some("confirmed,paid")), None);
        assert_eq!(statuses(Some("")), None);
        assert_eq!(limit(Some(0)), None);
        assert_eq!(limit(Some(MAX_LIMIT + 1)), None);
        assert_eq!(offset(Some(-1)), None);
    }

    /// The boundary this exists for: a list that is exactly one full window has no next
    /// one, and a list one row longer does.
    #[test]
    fn a_full_last_window_does_not_promise_an_empty_one() {
        assert_eq!(next_offset(0, 20, 20), None);
        assert_eq!(next_offset(0, 20, 21), Some(20));
        assert_eq!(next_offset(20, 20, 21), None);
        assert_eq!(next_offset(0, 2, 0), None);
    }
}
