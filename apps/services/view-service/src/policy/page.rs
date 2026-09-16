//! One window of a list: how many rows, from where, and what comes next.
//!
//! Every list view-service serves is paged the same way — the host's spots, the
//! renter's spots, and the bookings on a spot for either — so the three rules live once.
//! They are exactly the kind of thing that is wrong in ways nobody notices: a window
//! that starts one row late, a "next offset" that points past the end and renders empty,
//! an offset that goes negative and takes the statement with it.
//!
//! Every parser answers `None` for a request that is not a window of a list, and the
//! service turns any `None` into one 422. Nothing is clamped: a limit of 500 means the
//! caller wants something no endpoint here serves, and silently serving 50 would hide
//! that.

/// Rows when the caller does not say. What the paged screens read.
pub const DEFAULT_LIMIT: i64 = 20;
/// The most one request may ask for.
pub const MAX_LIMIT: i64 = 50;

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

/// `(limit, offset)` for a request, or `None` if either is out of range.
pub fn window(limit_param: Option<i64>, offset_param: Option<i64>) -> Option<(i64, i64)> {
    Some((limit(limit_param)?, offset(offset_param)?))
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
    fn absent_parameters_are_the_first_window() {
        assert_eq!(limit(None), Some(DEFAULT_LIMIT));
        assert_eq!(offset(None), Some(0));
    }

    #[test]
    fn parses_what_the_screens_send() {
        assert_eq!(limit(Some(2)), Some(2));
        assert_eq!(limit(Some(MAX_LIMIT)), Some(MAX_LIMIT));
        assert_eq!(offset(Some(40)), Some(40));
    }

    /// Each of these is the caller asking for something that is not a window of a list.
    #[test]
    fn rejects_rather_than_clamps() {
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
