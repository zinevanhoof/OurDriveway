//! How much of a host's balance a withdrawal may actually take.
//!
//! Two numbers and no I/O: what was asked for, and what was available at the moment the
//! balance was read. Everything that makes those numbers *trustworthy* — the advisory
//! lock, and the balance query running inside the transaction below it — is
//! `PaymentService::request_payout`'s job, not this file's.
//!
//! The upper bound could not live in `shared`'s request validator for exactly that
//! reason: a maximum is a fact about the database at one instant, and a validator sees
//! only the body.

use shared::domain_models::payment::payout::MIN_CENTS;

/// Why a withdrawal was refused. A return value, not state — see `policy/mod.rs`.
#[derive(Debug, PartialEq, Eq)]
pub enum Rejection {
    /// Nothing has settled yet, so there is no withdrawal to make at any size. Kept
    /// apart from `BelowMinimum` because it is the honest answer to a *double-clicked*
    /// withdrawal: the loser of the advisory lock re-reads and finds zero.
    NothingAvailable,
    /// Under €10. The client mirrors this rule in the form, so reaching it means a
    /// hand-written request or a stale page.
    BelowMinimum,
    /// More than has settled. The commonest real cause is not an attack but time: a
    /// booking that counted when the page was drawn can be refunded before the request
    /// lands.
    AboveAvailable { available_cents: i64 },
}

/// What this withdrawal pays out, or why it pays out nothing.
///
/// Deliberately **not** a clamp. Silently paying out €40 to someone who asked for €50
/// would answer a different question than the one the screen asked, and the client has
/// no way to tell it happened; the refusal names the figure instead and lets the screen
/// re-offer it. What the server does reserve is the right to compute `available_cents`
/// itself — this function never sees what the client believed.
pub fn check(requested_cents: i64, available_cents: i64) -> Result<i64, Rejection> {
    if available_cents <= 0 {
        return Err(Rejection::NothingAvailable);
    }
    if requested_cents < MIN_CENTS {
        return Err(Rejection::BelowMinimum);
    }
    if requested_cents > available_cents {
        return Err(Rejection::AboveAvailable { available_cents });
    }
    Ok(requested_cents)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_whole_balance_is_withdrawable() {
        assert_eq!(check(5_000, 5_000), Ok(5_000));
    }

    #[test]
    fn the_minimum_is_inclusive() {
        assert_eq!(check(MIN_CENTS, 5_000), Ok(MIN_CENTS));
        assert_eq!(check(MIN_CENTS - 1, 5_000), Err(Rejection::BelowMinimum));
    }

    #[test]
    fn nothing_over_the_balance_gets_out() {
        assert_eq!(
            check(5_001, 5_000),
            Err(Rejection::AboveAvailable {
                available_cents: 5_000
            })
        );
    }

    /// An empty balance is its own answer at every size — including one under the
    /// minimum, where "the smallest withdrawal is €10" would be a misleading thing to
    /// tell someone who has nothing.
    #[test]
    fn an_empty_balance_says_so_first() {
        for requested in [1, MIN_CENTS, 100_000] {
            assert_eq!(check(requested, 0), Err(Rejection::NothingAvailable));
        }
    }

    /// Withdrawing everything and then having a booking refunded goes negative — see
    /// `Earnings::available_cents`. It must read as nothing available, never as a
    /// balance to compare against.
    #[test]
    fn an_overdrawn_balance_is_nothing_available() {
        assert_eq!(check(MIN_CENTS, -500), Err(Rejection::NothingAvailable));
    }

    /// The one that would be a real bug: a negative or zero request must never reach
    /// the transfer. `BelowMinimum` catches it, and the ordering above is what makes
    /// that true — the available check runs first, so this only fires with money in the
    /// account.
    #[test]
    fn nothing_absurd_gets_through() {
        for requested in [i64::MIN, -1, 0] {
            assert_eq!(check(requested, 5_000), Err(Rejection::BelowMinimum));
        }
    }
}
