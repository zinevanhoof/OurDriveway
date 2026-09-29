//! How often one account may be sent a verification or password-reset email.
//!
//! Both endpoints are unauthenticated and each call is a paid email to someone's inbox,
//! so without this anyone could mail-bomb an address from a loop. The per-IP limit at
//! the ingress does not cover it: a botnet spreads the loop across IPs, and this counts
//! per *account*.
//!
//! The caller answers exactly as it would have after a send — the same response for a
//! suppressed mail, a sent one, and an address with no account behind it — so the
//! cooldown reveals nothing about whether an account exists.

use chrono::{DateTime, TimeDelta, Utc};

/// At most one account email per this long. Short enough that someone whose first mail
/// went astray can ask again soon; long enough that a loop gets one mail every two
/// minutes instead of one per request.
pub const COOLDOWN: TimeDelta = TimeDelta::minutes(2);

/// Whether a mail may go out now, given when the last one did.
pub fn may_send(last: Option<DateTime<Utc>>, now: DateTime<Utc>) -> bool {
    last.is_none_or(|last| now - last >= COOLDOWN)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(secs: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(1_800_000_000 + secs, 0).unwrap()
    }

    #[test]
    fn the_first_mail_always_goes() {
        assert!(may_send(None, at(0)));
    }

    #[test]
    fn a_second_mail_inside_the_cooldown_does_not() {
        assert!(!may_send(Some(at(0)), at(1)));
        assert!(!may_send(Some(at(0)), at(COOLDOWN.num_seconds() - 1)));
    }

    #[test]
    fn the_cooldown_ends_exactly_on_time() {
        assert!(may_send(Some(at(0)), at(COOLDOWN.num_seconds())));
    }

    /// A clock that stepped backwards leaves a `last` in the future. That must hold mail
    /// back only until `last + COOLDOWN`, not lock the account out of it for good.
    #[test]
    fn a_last_send_in_the_future_blocks_only_until_its_cooldown_ends() {
        let last = at(60);
        assert!(!may_send(Some(last), at(0)));
        assert!(may_send(Some(last), last + COOLDOWN));
    }
}
