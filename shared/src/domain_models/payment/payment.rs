use surrealdb::types::vars;
use surrealdb::types::{Datetime, SurrealValue};
use surrealdb::{engine::remote::ws::Client, method::Query};
use uuid::Uuid;

use crate::events::payment::PaymentCreated;

/// Where a payment is, as stored in `status`.
///
/// Bare `&str` because that is what the column is and what every guard compares
/// against; the schema's `ASSERT $value IN [...]` is the authority. Written once
/// here so a typo is a compile error rather than a transition that silently never
/// matches.
pub mod status {
    /// A session exists and nothing has been charged.
    pub const CREATED: &str = "created";
    /// Money taken, booking confirmed.
    pub const SUCCEEDED: &str = "succeeded";
    /// An attempt was declined. **Not terminal** — the renter may confirm the same
    /// session again with another method, which is why it does not stop `settle_up`
    /// from expiring the session later. It exists so the checkout screen can say
    /// what happened rather than reconstruct it from `created` plus a non-empty
    /// `failure_reason`.
    pub const FAILED: &str = "failed";
    /// Taken and given back.
    pub const REFUNDED: &str = "refunded";
    /// The session was voided before anyone paid.
    pub const EXPIRED: &str = "expired";

    /// The two states an unpaid session can be in. Every transition out of "nobody
    /// has paid yet" accepts both, because a declined attempt leaves the session as
    /// live as an untouched one.
    pub const UNPAID: [&str; 2] = [CREATED, FAILED];
}

/// The `payment` table, whole.
///
/// Read entire rather than per-use-case: this replaced a `PaymentRow` that selected
/// ten of the eleven columns anyway, on a row always addressed by a unique index.
#[derive(Clone, Debug, SurrealValue)]
pub struct Payment {
    pub id: Uuid,
    /// `payment_booking … UNIQUE`, and load-bearing: one booking gets at most one
    /// PaymentIntent, so a second row for one booking means the renter could be
    /// charged twice.
    pub booking_id: Uuid,
    /// Who earns it. Denormalised off the event so the earnings query never
    /// dereferences a booking.
    pub owner_id: Uuid,
    /// Who paid. Read so a session lookup can be scoped to the asking renter before
    /// Stripe is asked anything.
    pub renter_id: Uuid,
    /// EUR cents, as the server priced the booking at reserve time.
    pub amount_cents: i64,
    /// `cs_…`, known the moment the session is created. `payment_session … UNIQUE`.
    /// This is what expires an unpaid checkout — at that point no intent exists.
    pub session_id: String,
    /// `pi_…`, NONE until the payment succeeds. What a refund is issued against,
    /// because Stripe's refund API takes an intent or a charge and never a session.
    ///
    /// Written in the *same patch* as `status = 'succeeded'` — see
    /// [`PaymentPatch::succeeded`].
    pub intent_id: Option<String>,
    /// The shard this payment's subject lives on, stored rather than recomputed:
    /// `shard_of` is deterministic only for a fixed SHARD_COUNT, and raising it
    /// would split one payment's history across two subjects.
    pub booking_shard: String,
    /// One of [`status`].
    pub status: String,
    /// Set together with `status = 'refunded'`. Its presence is what makes the
    /// refund path idempotent — `settle_up` refuses a payment that already has one.
    pub refund_id: Option<String>,
    /// Stripe's message from the last failed attempt. Kept for the log, never
    /// rendered to a renter — the Payment Element already told them, in their
    /// language.
    pub failure_reason: Option<String>,
    pub created_at: Datetime,
}

impl Payment {
    /// The row a `Created` writes. Everything else is a patch on top of this.
    pub fn created(e: PaymentCreated) -> Self {
        Self {
            id: e.payment_id,
            booking_id: e.booking_id,
            owner_id: e.owner_id,
            renter_id: e.renter_id,
            amount_cents: e.amount_cents,
            session_id: e.session_id,
            intent_id: None,
            booking_shard: e.booking_shard,
            status: status::CREATED.to_string(),
            refund_id: None,
            failure_reason: None,
            // The event's own timestamp, not this process's: every replica must
            // store the same one.
            created_at: e.created_at.into(),
        }
    }
}

/// A partial update to a [`Payment`], written with the struct-update idiom:
///
/// ```ignore
/// PaymentPatch { status: Some(hash), ..Default::default() }
/// ```
///
/// Only the four columns anything actually patches. The other eight are written
/// once by [`Payment::created`] and never change, so they are not representable
/// here — which is a little stronger than the all-columns version this replaced.
#[derive(Debug, Default)]
pub struct PaymentPatch {
    pub status: Option<String>,
    pub intent_id: Option<String>,
    pub refund_id: Option<String>,
    pub failure_reason: Option<String>,
}

impl PaymentPatch {
    /// Binds every patchable column. Absent ones bind as NONE, which the `?? column`
    /// in `PaymentRepository::transition` turns into "leave it alone".
    ///
    /// Every field here has to appear in that statement's `SET` list, and vice
    /// versa. Nothing checks that at compile time — `set_covers_every_patchable_column`
    /// is the reminder, and the live round-trip in payment-service is what would
    /// actually catch a mismatch.
    pub fn bind(self, q: Query<'_, Client>) -> Query<'_, Client> {
        q.bind(vars! {
            status:         self.status,
            intent_id:      self.intent_id,
            refund_id:      self.refund_id,
            failure_reason: self.failure_reason,
        })
    }

    /// Paid — and the one place `intent_id` becomes known.
    ///
    /// The two are set together deliberately. `settle_up`'s refund arm matches
    /// `status = 'succeeded'` and then needs `intent_id`, so a row with one and not
    /// the other is a refund it could not issue. One patch makes that state
    /// unrepresentable rather than merely unlikely.
    pub fn succeeded(intent_id: String) -> Self {
        Self {
            status: Some(status::SUCCEEDED.to_string()),
            intent_id: Some(intent_id),
            ..Self::default()
        }
    }

    pub fn failed(reason: String) -> Self {
        Self {
            status: Some(status::FAILED.to_string()),
            failure_reason: Some(reason),
            ..Self::default()
        }
    }

    /// Refunded, carrying the id that stops it happening twice.
    pub fn refunded(refund_id: String) -> Self {
        Self {
            status: Some(status::REFUNDED.to_string()),
            refund_id: Some(refund_id),
            ..Self::default()
        }
    }

    pub fn expired() -> Self {
        Self {
            status: Some(status::EXPIRED.to_string()),
            ..Self::default()
        }
    }
}

/// A host's settled income and what they have already withdrawn.
///
/// All derived, none stored. There is deliberately no balance column anywhere:
/// available is always `earned − paid out`, computed on read, which is what makes a
/// refunded booking drop out of a host's income for free instead of needing a
/// compensating write.
#[derive(Debug, Default, Clone, Copy)]
pub struct Earnings {
    pub earned_cents: i64,
    pub paid_out_cents: i64,
}

impl Earnings {
    pub fn available_cents(&self) -> i64 {
        self.earned_cents - self.paid_out_cents
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The SQL lives in `PaymentRepository` now, so nothing here can assert on it.
    /// What this *can* do is fail the moment the struct grows a field.
    ///
    /// The literal is **exhaustive on purpose** — no `..Default::default()`. Add a
    /// field to [`PaymentPatch`] and this stops compiling, which is the reminder
    /// that `bind` and the `SET` list in `transition` need it too.
    #[test]
    fn set_covers_every_patchable_column() {
        let _: PaymentPatch = PaymentPatch {
            status: None,
            intent_id: None,
            refund_id: None,
            failure_reason: None,
        };
    }

    /// The pairing the refund path leans on. A `succeeded` patch that could leave
    /// `intent_id` absent would produce a payment that cannot be refunded.
    #[test]
    fn succeeding_always_records_the_intent() {
        let patch = PaymentPatch::succeeded("pi_123".into());
        assert_eq!(patch.status.as_deref(), Some(status::SUCCEEDED));
        assert_eq!(patch.intent_id.as_deref(), Some("pi_123"));
    }

    /// Refunding records the id that makes a second refund impossible, and nothing
    /// else clears it — absent means unchanged.
    #[test]
    fn only_refunding_sets_a_refund_id() {
        assert_eq!(
            PaymentPatch::refunded("re_1".into()).refund_id.as_deref(),
            Some("re_1")
        );
        for patch in [
            PaymentPatch::succeeded("pi_1".into()),
            PaymentPatch::failed("declined".into()),
            PaymentPatch::expired(),
        ] {
            assert!(patch.refund_id.is_none());
        }
    }

    #[test]
    fn available_is_earned_minus_paid_out() {
        let e = Earnings {
            earned_cents: 5_000,
            paid_out_cents: 1_500,
        };
        assert_eq!(e.available_cents(), 3_500);
        // Withdrawing everything then having a booking refunded goes negative, and
        // that is the honest answer — `request_payout` refuses anything <= 0.
        let overdrawn = Earnings {
            earned_cents: 1_000,
            paid_out_cents: 1_500,
        };
        assert_eq!(overdrawn.available_cents(), -500);
    }
}
