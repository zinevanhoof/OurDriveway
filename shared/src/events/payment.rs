use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Everything that can happen to money. Published by payment-service only.
///
/// This log is the record of what Stripe was asked to do and what it answered. It is
/// deliberately not a mirror of Stripe's own events: `Succeeded` is published *after*
/// a webhook has been signature-verified and matched to a booking we know about, so
/// anything on this stream is already ours.
///
/// The one event that changes a *booking* is `Succeeded` — booking-service consumes
/// it and publishes `BookingEvent::Confirmed`. Nothing here writes booking state
/// directly; the two streams stay owned by one service each.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum PaymentEvent {
    /// A Checkout Session exists at Stripe and the renter has been handed its client
    /// secret. Nothing has been charged yet.
    Created(PaymentCreated),

    /// Stripe confirmed the charge, verified via the webhook signature.
    ///
    /// The point of no return: money has moved, so every consumer of this must
    /// succeed eventually rather than refuse. booking-service publishes `Confirmed`
    /// unconditionally on the strength of it.
    ///
    /// This is where `intent_id` enters the system. A session has no intent until it is
    /// paid, and a refund needs one — `CreateRefund` accepts an intent or a charge and
    /// never a session — so the handle arrives at exactly the moment it first becomes
    /// possible to need it.
    Succeeded {
        payment_id: Uuid,
        booking_id: Uuid,
        intent_id: String,
    },

    /// The renter's payment attempt failed. The hold is deliberately left alone —
    /// they may retry with another method, and it lapses on its own if they don't.
    Failed {
        payment_id: Uuid,
        booking_id: Uuid,
        /// Stripe's own message, carried verbatim for the log. Never shown to the
        /// renter as-is; the frontend has the same information from the Element.
        reason: String,
    },

    /// Money returned in full, because a paid booking was withdrawn or a payment
    /// landed after its hold had already lapsed.
    Refunded {
        payment_id: Uuid,
        booking_id: Uuid,
        refund_id: String,
        amount_cents: i64,
        /// When the money went back. Carried rather than left to the projector's
        /// clock for the same reason as `PayoutRequested::requested_at`: every
        /// replica applies this independently and must date it identically.
        ///
        /// It is a separate instant from the payment's `created_at` — that is when
        /// the checkout session was made — and a wallet groups the two into
        /// different months whenever a booking is cancelled after the turn of one.
        refunded_at: DateTime<Utc>,
    },

    /// An unpaid Checkout Session was expired because its booking ended before anyone
    /// paid for it.
    ///
    /// Worth an event rather than silence: an expired session cannot be confirmed, which
    /// is what stops a renter paying for a hold they already lost, and the resulting
    /// `expired` row is what stops `settle_up` trying again.
    ///
    /// Keyed on the session rather than the intent on purpose — this is the one path
    /// that has to work *before* a payment exists, and at that point the session id is
    /// the only Stripe handle there is.
    SessionExpired { payment_id: Uuid, booking_id: Uuid },

    /// A host asked to withdraw. **Nothing has moved yet** — this is the request, and
    /// the transfer it causes is made by a worker off this very event.
    ///
    /// `amount_cents` is computed server-side: a client may ask for a figure, but it is
    /// only ever narrowed by earnings minus prior payouts, read under the advisory lock
    /// inside the transaction that publishes this. What a client sent never reaches here.
    PayoutRequested {
        payout_id: Uuid,
        /// The host.
        host_id: Uuid,
        amount_cents: i64,
        requested_at: DateTime<Utc>,
    },

    /// The Stripe Transfer succeeded: the money is in the host's connected account.
    ///
    /// `transfer_id` is carried for the same reason `Refunded` carries `refund_id` —
    /// it is the handle a reversal would need, and its presence in the row is the
    /// second reading of "already paid".
    PayoutPaid {
        payout_id: Uuid,
        /// Carried so this lands on the same `payments.payout.<host>` subject as the
        /// request, which is what keeps one payout's three events in order.
        host_id: Uuid,
        transfer_id: String,
        /// Off the event, never a projector's clock — every replica must date it the
        /// same. Same rule as `Refunded::refunded_at`.
        paid_at: DateTime<Utc>,
    },

    /// Stripe refused the transfer, permanently. The commonest cause in test mode is
    /// `balance_insufficient` — charges land in the platform's *pending* balance and a
    /// transfer can only draw on the available one.
    ///
    /// The money is not lost: `total_for` sums only requested and paid payouts, so a
    /// failed row drops straight back out of the balance.
    PayoutFailed {
        payout_id: Uuid,
        host_id: Uuid,
        /// Stripe's message, for the log. Never rendered to a host — a failed payout is
        /// filtered out of the wallet entirely.
        reason: String,
        failed_at: DateTime<Utc>,
    },
}

impl PaymentEvent {
    /// The payment a variant is about — the aggregate half of `payment:<uuid>`.
    ///
    /// `None` for the three payout variants, which concern no payment at all: they are
    /// their own `payout:<uuid>` aggregate, written to a different table.
    pub fn payment_id(&self) -> Option<Uuid> {
        match self {
            Self::Created(e) => Some(e.payment_id),
            Self::Succeeded { payment_id, .. }
            | Self::Failed { payment_id, .. }
            | Self::Refunded { payment_id, .. }
            | Self::SessionExpired { payment_id, .. } => Some(*payment_id),
            Self::PayoutRequested { .. } | Self::PayoutPaid { .. } | Self::PayoutFailed { .. } => {
                None
            }
        }
    }

    /// The payout a variant is about — the aggregate half of `payout:<uuid>`.
    ///
    /// The mirror of [`Self::payment_id`], and the reason it exists is the projector:
    /// applying one of these has to find the row, and matching the three variants in
    /// every consumer that only needs the id is three arms each time.
    pub fn payout_id(&self) -> Option<Uuid> {
        match self {
            Self::PayoutRequested { payout_id, .. }
            | Self::PayoutPaid { payout_id, .. }
            | Self::PayoutFailed { payout_id, .. } => Some(*payout_id),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PaymentCreated {
    pub payment_id: Uuid,
    /// Selects the subject this payment lives on: one booking's payment history
    /// (created, succeeded, refunded) stays on a single ordered subject.
    pub booking_id: Uuid,
    /// The host who earns this. Denormalized so the earnings query is one indexed
    /// scan of `payment` and never joins back through the booking projection.
    pub host_id: Uuid,
    /// From the verified JWT claim. Who is paying.
    pub renter_id: Uuid,
    /// Stripe's `cs_…`. Known as soon as the session exists, which is what makes it the
    /// handle for expiring an unpaid checkout — the intent does not exist yet.
    pub session_id: String,
    /// EUR cents, taken from the booking as priced server-side at reserve time. The
    /// client never supplies this.
    pub amount_cents: i64,
    /// Set here rather than read from a clock in the projector: every replica
    /// replays this event independently and must agree.
    pub created_at: DateTime<Utc>,
}
