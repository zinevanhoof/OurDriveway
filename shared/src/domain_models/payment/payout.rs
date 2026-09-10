use chrono::{DateTime, Utc};
use diesel::prelude::*;
use uuid::Uuid;

use crate::events::payment::PaymentEvent;

/// The smallest withdrawal, in EUR cents.
///
/// Here rather than in payment-service's `policy::payout` because two places need the
/// same number and they are in different crates: the request validator in
/// `shared::requests::payment` rejects anything smaller with a 422 before a handler
/// runs, and the policy checks it again against the balance read under the lock. The
/// second is the one that is load-bearing — a validator is a courtesy to the form.
pub const MIN_CENTS: i64 = 1_000;

/// Where a payout is, as stored in `status`.
///
/// Bare `&str` for the same reason as [`super::payment::status`]: that is what the
/// column is, the schema's `CHECK (status IN (…))` is the authority, and spelling each
/// one once makes a typo a compile error rather than a transition that never matches.
pub mod status {
    /// Written, published, and waiting for the worker to make the transfer. The money
    /// is already out of the host's available balance at this point — see
    /// [`super::Payout`].
    pub const REQUESTED: &str = "requested";
    /// The Stripe Transfer landed in the host's connected account.
    pub const PAID: &str = "paid";
    /// Stripe refused it. Terminal, and the money comes back by this row dropping out
    /// of the sum rather than by any compensating write.
    pub const FAILED: &str = "failed";

    /// The two that still count against a host's balance. `PayoutRepository::total_for`
    /// and view-service's wallet queries must both use exactly this set — they are the
    /// same arithmetic over two databases, and disagreeing is the one way this design
    /// can drift.
    pub const COUNTED: [&str; 2] = [REQUESTED, PAID];
}

/// A host withdrawing their balance, as a Stripe Transfer from the platform account to
/// their connected one.
///
/// It used to be pure bookkeeping — no Connect account, no transfer, no bank — and the
/// row existed only so the balance went down and stayed down. It is real now, which is
/// what the three [`status`] values are for: the request commits, and a worker moves it
/// to `paid` or `failed` a moment later.
///
/// There is still deliberately no stored balance to decrement: available is always
/// `earnings − Σ payouts that count`, computed on read. That is what makes both a
/// refunded booking and a *failed* payout drop out of a host's figures for free, with
/// no compensating write on either side.
#[derive(Clone, Debug, Queryable, Selectable, Insertable, AsChangeset)]
#[diesel(table_name = crate::schema::payment::payout)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct Payout {
    pub id: Uuid,
    /// Bumped on every transition now, where it used to be written once — the worker's
    /// outcome is a second event for this aggregate, and the client waits on the
    /// version the request reached.
    pub version: i64,
    pub host_id: Uuid,
    pub amount_cents: i64,
    /// One of [`status`].
    pub status: String,
    /// `tr_…`, NONE until the transfer succeeds. Written in the same patch as
    /// `status = 'paid'`.
    pub transfer_id: Option<String>,
    /// Stripe's message from a refusal. Kept for the log; a failed payout is filtered
    /// out of the wallet, so this is never rendered to a host.
    pub failure_reason: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl Payout {
    /// The row a `PayoutRequested` writes, or `None` for any other payment event.
    ///
    /// Takes the whole event rather than the four fields so the projector's arm
    /// stays a single line, and so the mapping lives next to the row it produces.
    pub fn requested(event: &PaymentEvent, version: i64) -> Option<Self> {
        match event {
            PaymentEvent::PayoutRequested {
                payout_id,
                host_id,
                amount_cents,
                requested_at,
            } => Some(Self {
                id: *payout_id,
                version,
                host_id: *host_id,
                amount_cents: *amount_cents,
                status: status::REQUESTED.to_string(),
                transfer_id: None,
                failure_reason: None,
                // The requester's timestamp off the event, not this replica's clock.
                created_at: *requested_at,
            }),
            _ => None,
        }
    }
}

/// A partial update to a [`Payout`], written with the struct-update idiom — the same
/// shape as [`super::payment::PaymentPatch`], for the same reason.
///
/// Only the three columns the worker's outcome touches. Everything else is written once
/// by [`Payout::requested`] and is not representable here: an amount that could be
/// patched is an amount a bug could raise after the balance check that authorised it.
#[derive(Debug, Default, AsChangeset)]
#[diesel(table_name = crate::schema::payment::payout)]
pub struct PayoutPatch {
    pub status: Option<String>,
    pub transfer_id: Option<String>,
    pub failure_reason: Option<String>,
}

impl PayoutPatch {
    /// Paid — and the one place `transfer_id` becomes known.
    ///
    /// Set together deliberately, exactly as `PaymentPatch::succeeded` pairs its status
    /// with `intent_id`: a paid payout with no transfer id is one nothing could ever
    /// reverse or reconcile against Stripe.
    pub fn paid(transfer_id: String) -> Self {
        Self {
            status: Some(status::PAID.to_string()),
            transfer_id: Some(transfer_id),
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
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Exhaustive on purpose — no `..Default::default()`. Add a field to [`PayoutPatch`]
    /// and this stops compiling, which is the reminder that the `SET` list in
    /// `PayoutRepository::transition` needs it too, and a `.bind()` in the matching
    /// position. Same contract as `set_covers_every_patchable_column` on payments.
    #[test]
    fn set_covers_every_patchable_column() {
        let _: PayoutPatch = PayoutPatch {
            status: None,
            transfer_id: None,
            failure_reason: None,
        };
    }

    /// The pairing every reconciliation leans on.
    #[test]
    fn paying_always_records_the_transfer() {
        let patch = PayoutPatch::paid("tr_123".into());
        assert_eq!(patch.status.as_deref(), Some(status::PAID));
        assert_eq!(patch.transfer_id.as_deref(), Some("tr_123"));
        assert!(PayoutPatch::failed("no funds".into()).transfer_id.is_none());
    }

    /// The set the balance is computed over. A failed payout must not count, or the
    /// money never comes back; a requested one must, or a host can withdraw it twice
    /// while the transfer is in flight.
    #[test]
    fn only_failed_payouts_stop_counting() {
        assert!(status::COUNTED.contains(&status::REQUESTED));
        assert!(status::COUNTED.contains(&status::PAID));
        assert!(!status::COUNTED.contains(&status::FAILED));
    }
}
