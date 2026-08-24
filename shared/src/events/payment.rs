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

    /// A host withdrew their balance. Nothing leaves any real account — see the note
    /// on `payout` in schemas/payment-schema.surql.
    ///
    /// `amount_cents` is computed server-side from earnings minus prior payouts. A
    /// client-supplied figure never reaches this.
    PayoutRequested {
        payout_id: Uuid,
        /// The host.
        owner_id: Uuid,
        amount_cents: i64,
        requested_at: DateTime<Utc>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PaymentCreated {
    pub payment_id: Uuid,
    pub booking_id: Uuid,
    /// `shard_of(&booking_id)`, echoed on every later event for this payment so the
    /// subject is never recomputed — the same reasoning as `spot_shard` on
    /// `BookingCreated`.
    pub booking_shard: String,
    /// The host who earns this. Denormalized so the earnings query is one indexed
    /// scan of `payment` and never joins back through the booking projection.
    pub owner_id: Uuid,
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
