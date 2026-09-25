//! Which bookings a list of them holds: the tab and the statuses.
//!
//! The window itself — limit, offset, what comes next — is [`super::page`], shared with
//! every other list. What is here is only what a *booking* list adds on top: which side
//! of now, and which statuses. Both are parsed to `None` for a request that is not a
//! booking list, and the service turns that into the same 422 as a bad window.

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

/// `(past, statuses, limit, offset)` for a booking list request, or `None` if any of the
/// four is not something a booking list serves.
pub fn window(
    scope_param: Option<&str>,
    status_param: Option<&str>,
    limit: Option<i64>,
    offset: Option<i64>,
) -> Option<(bool, Vec<&'static str>, i64, i64)> {
    let (limit, offset) = super::page::window(limit, offset)?;
    Some((scope(scope_param)?, statuses(status_param)?, limit, offset))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_parameters_are_every_upcoming_booking() {
        assert_eq!(scope(None), Some(false));
        assert_eq!(statuses(None), Some(STATUSES.to_vec()));
    }

    #[test]
    fn parses_what_the_screens_send() {
        assert_eq!(scope(Some("past")), Some(true));
        assert_eq!(statuses(Some("confirmed")), Some(vec!["confirmed"]));
        assert_eq!(
            statuses(Some("cancelled,released")),
            Some(vec!["cancelled", "released"])
        );
    }

    /// Each of these is the caller asking for something that is not a booking list.
    #[test]
    fn rejects_rather_than_clamps() {
        assert_eq!(scope(Some("cancelled")), None);
        assert_eq!(statuses(Some("paid")), None);
        assert_eq!(statuses(Some("confirmed,paid")), None);
        assert_eq!(statuses(Some("")), None);
    }
}
