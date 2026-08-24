//! What happens to money when a booking ends.
//!
//! One rule, reached from two directions: a booking can end before its payment
//! resolves, and a payment can resolve after its booking has ended. Both `Worker`s
//! route through `SettlementWorkerService::settle_up`, which decides from current
//! state rather than from which event woke it — and the deciding is this file.

/// The whole money rule, as data.
#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    /// Money was taken and the booking is not happening. Give it back.
    Refund,
    /// A checkout exists that nobody paid, and nobody will. Void it, so a renter cannot
    /// complete a payment for a hold they have already lost.
    ///
    /// Acts on the *session*, which is the only Stripe handle that exists before a
    /// payment does — a session has no PaymentIntent until it is paid.
    ExpireSession,
    /// Either the booking is still live, or this payment is already settled.
    Nothing,
}

/// The decision, with no I/O in it.
///
/// `booking_status` and `payment_status` are the projected states; `refunded` is
/// whether a refund id is already recorded.
///
/// `failed` and `created` are deliberately the same case. A failed attempt is not
/// terminal — the renter can confirm the same session again with another method — so for
/// the purposes of *money*, an unpaid session is an unpaid session however many times
/// someone has tried. `failed` exists so the checkout screen can say what happened, not
/// because it changes what settling does.
pub fn decide(booking_status: &str, payment_status: &str, refunded: bool) -> Action {
    // Belt and braces with the `WHERE status = 'succeeded'` guard in the projector: a
    // payment that already carries a refund id is never refunded twice, whatever its
    // status says.
    if refunded {
        return Action::Nothing;
    }

    match booking_status {
        // Still in play. A reserved booking may yet be paid; a confirmed one is the
        // outcome we want and the host has earned it.
        "reserved" | "confirmed" => Action::Nothing,

        // The booking is over without being honoured — the renter backed out, the hold
        // lapsed, they cancelled in time, or the host withdrew the spot. All four are
        // the same question for money.
        "released" | "cancelled" => match payment_status {
            "succeeded" => Action::Refund,
            "created" | "failed" => Action::ExpireSession,
            // 'refunded' and 'expired' are terminal; anything unrecognised is left
            // alone rather than guessed at.
            _ => Action::Nothing,
        },

        _ => Action::Nothing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The decision table from the plan, exhaustively. This is the money rule; if it
    /// is wrong the system either keeps a renter's money or refunds a host's income.
    #[test]
    fn the_decision_table_holds() {
        // A live booking never settles, whatever the payment is doing.
        for payment in ["created", "failed", "succeeded", "refunded", "expired"] {
            assert_eq!(
                decide("reserved", payment, false),
                Action::Nothing,
                "a reserved booking must not settle (payment {payment})"
            );
            assert_eq!(
                decide("confirmed", payment, false),
                Action::Nothing,
                "a confirmed booking is the good outcome; money stays (payment {payment})"
            );
        }

        // A booking that ended without being honoured.
        for ended in ["released", "cancelled"] {
            assert_eq!(
                decide(ended, "succeeded", false),
                Action::Refund,
                "{ended} with money taken must refund"
            );
            assert_eq!(
                decide(ended, "created", false),
                Action::ExpireSession,
                "{ended} with an unpaid checkout must void the session"
            );
            // A declined attempt leaves a *live* session, so it still has to be voided —
            // 'failed' is a fact about the last try, not a terminal state.
            assert_eq!(
                decide(ended, "failed", false),
                Action::ExpireSession,
                "{ended} after a declined attempt must still void the session"
            );
            // Already settled, both ways.
            assert_eq!(decide(ended, "refunded", false), Action::Nothing);
            assert_eq!(decide(ended, "expired", false), Action::Nothing);
        }
    }

    /// The double-refund guard, which is the expensive mistake in this file. A
    /// redelivered `Cancelled` after a successful refund must not refund again.
    #[test]
    fn a_refunded_payment_is_never_refunded_twice() {
        assert_eq!(
            decide("cancelled", "succeeded", true),
            Action::Nothing,
            "a recorded refund id must stop a second refund even if status still reads succeeded"
        );
        assert_eq!(decide("released", "succeeded", true), Action::Nothing);
    }

    /// An unknown status is left alone rather than guessed at — if booking-service ever
    /// adds a state, this file should do nothing until it is taught what it means.
    #[test]
    fn unknown_statuses_do_nothing() {
        assert_eq!(decide("completed", "succeeded", false), Action::Nothing);
        assert_eq!(decide("released", "pending_review", false), Action::Nothing);
    }
}
