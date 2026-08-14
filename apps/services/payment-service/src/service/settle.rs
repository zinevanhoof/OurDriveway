//! What happens to money when a booking ends.
//!
//! One rule, reached from two directions. A booking can end before its payment
//! resolves, and a payment can resolve after its booking has ended, so both the
//! BOOKINGS worker and the PAYMENTS worker call [`Settler::settle_up`] and it decides
//! from current state rather than from which event woke it.

use async_nats::jetstream::Context;
use chrono::Utc;
use shared::{
    error::myerror::MyResult,
    events::{Envelope, payment::PaymentEvent, payment_subject, user::record_key},
};
use std::sync::Arc;
use uuid::Uuid;

use crate::{repository::payment_repository::PaymentRepository, service::stripe::Stripe};

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

pub struct Settler {
    pub repository: Arc<PaymentRepository>,
    pub stripe: Arc<Stripe>,
    pub js: Context,
}

impl Settler {
    /// Applies [`decide`] to one booking and publishes the result.
    ///
    /// Both callers are `Worker`s, so an `Err` here becomes a NAK and comes back in
    /// thirty seconds. That retry is load-bearing rather than incidental: the two
    /// projections advance independently, so this can legitimately run while one side
    /// is a moment stale. Cancelling an intent Stripe has already captured fails at
    /// Stripe, the message is redelivered, and by then the projection shows
    /// `succeeded` and the same call refunds instead. The reconciliation *is* the
    /// retry — there is deliberately no polling loop anywhere.
    pub async fn settle_up(&self, booking_id: &Uuid) -> MyResult<()> {
        let key = record_key(booking_id);

        // A payment we have never heard of is the common case, not a problem: most
        // bookings are released without anyone reaching checkout.
        let Some(payment) = self.repository.payment_for_booking(&key).await? else {
            return Ok(());
        };

        // The booking, on the other hand, must exist — this payment was created from
        // it. Missing means our own BOOKINGS projection is behind, so fail and let the
        // redelivery find it rather than silently skipping a refund.
        let booking = self.repository.booking(&key).await?.ok_or_else(|| {
            shared::error::myerror::MyError::Bus(format!(
                "booking {key} not projected yet; retrying"
            ))
        })?;

        let payment_id: Uuid = payment
            .id
            .parse()
            .map_err(|e| shared::error::myerror::MyError::Bus(format!("payment id: {e}")))?;

        let event = match decide(&booking.status, &payment.status, payment.refund_id.is_some()) {
            Action::Nothing => return Ok(()),

            Action::Refund => {
                // Non-null by construction: `intent_id` is written in the same statement
                // as `status = 'succeeded'`, and that status is the only one this arm
                // matches. Erroring rather than unwrapping so a future change that breaks
                // the pairing surfaces as a retry, not a panic in a money path.
                let intent_id = payment.intent_id.as_deref().ok_or_else(|| {
                    shared::error::myerror::MyError::Bus(format!(
                        "payment {payment_id} is succeeded but has no intent id"
                    ))
                })?;

                tracing::info!(%booking_id, status = %booking.status, "refunding");
                let refund_id = self.stripe.refund(intent_id, &payment_id).await?;
                PaymentEvent::Refunded {
                    payment_id,
                    booking_id: *booking_id,
                    refund_id,
                    amount_cents: payment.amount_cents,
                }
            }

            Action::ExpireSession => {
                tracing::info!(%booking_id, status = %booking.status, "expiring unpaid session");
                self.stripe.expire_session(&payment.session_id).await?;
                PaymentEvent::SessionExpired {
                    payment_id,
                    booking_id: *booking_id,
                }
            }
        };

        // Deterministic event id: a redelivery that gets this far — because the Stripe
        // call succeeded but the publish or the ack did not — is discarded by the
        // stream's duplicate window rather than recorded twice.
        let mut envelope = Envelope::new(event, None);
        envelope.event_id = Uuid::new_v5(
            &Uuid::NAMESPACE_OID,
            format!("settle:{payment_id}:{}", booking.status).as_bytes(),
        );

        // No compare-and-swap: one payment owns this subject, so there is no second
        // writer to race.
        bus::publish(
            &self.js,
            payment_subject(&payment.booking_shard, booking_id),
            &envelope,
        )
        .await?;

        Ok(())
    }

    /// Whether a host may withdraw, and how much. Derived, never stored.
    pub async fn available_for(&self, owner_id: &str, settlement_secs: i64) -> MyResult<i64> {
        let cutoff = Utc::now() - chrono::Duration::seconds(settlement_secs);
        Ok(self
            .repository
            .earnings(owner_id, cutoff)
            .await?
            .available_cents())
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
