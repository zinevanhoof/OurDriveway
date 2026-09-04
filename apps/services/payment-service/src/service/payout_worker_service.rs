//! Making the withdrawal actually happen: platform balance → the host's connected
//! account.
//!
//! Stream-driven, hence `*_worker_service.rs`. There is no caller to authorize and no
//! request to validate — that already happened, under a lock, in
//! `PaymentService::request_payout`. By the time this runs the decision is made and
//! committed; what is left is the side effect and recording what came back.
//!
//! # Why the Stripe call is not in the request
//!
//! `request_payout` holds `pg_advisory_xact_lock` on the host for the whole
//! transaction. A Stripe call inside it would hold that lock across a network round
//! trip, and a timeout would leave the money moved with no row to show for it. Out
//! here the transaction is already committed, a failure is a NAK that comes back in
//! thirty seconds, and the idempotency key means the retry cannot pay twice.

use std::sync::Arc;

use bus::outbox;
use chrono::Utc;
use shared::db;
use shared::{
    domain_models::payment::{PayoutPatch, payout::status},
    error::myerror::{MyError, MyResult},
    events::{Envelope, aggregate_id, payment::PaymentEvent, payout_subject},
};
use uuid::Uuid;

use crate::{
    client::stripe::{Stripe, Transferred},
    repository::{
        connect_account_repository::ConnectAccountRepository,
        payout_repository::PayoutRepository,
    },
};

pub struct PayoutWorkerService {
    /// The pool. See the note on `PaymentService::db` — the repositories are stateless.
    pub db: sqlx::PgPool,
    pub stripe: Arc<Stripe>,
}

impl PayoutWorkerService {
    /// Transfers one requested payout, and publishes what happened.
    ///
    /// # The three things that stop this paying twice
    ///
    /// 1. **The status guard**, first and cheapest: anything but `requested` returns
    ///    immediately. That covers the ordinary redelivery, where our own commit
    ///    landed and the ack did not.
    /// 2. **The idempotency key** on the Stripe call, derived from the payout id. That
    ///    covers the case the guard cannot see — Stripe answered, we crashed before
    ///    committing, and the row still says `requested`. Stripe returns the first
    ///    transfer instead of making a second.
    /// 3. **The transition's own `WHERE status = 'requested'`**, which makes the write
    ///    a no-op if something moved the row while the transfer was in flight.
    ///
    /// An `Err` from here is a NAK: it comes back, and by then one of the three above
    /// is what makes the retry safe.
    ///
    /// ponytail: no reconciliation sweeper. A transfer Stripe made and we never
    /// recorded converges on the redelivery, because the idempotency key returns the
    /// same transfer — but if the stream ever loses the event, nothing goes looking.
    /// The fix is a periodic scan for `requested` rows older than a few minutes,
    /// checked against Stripe by idempotency key; it belongs in a `sweeper.rs`, which
    /// this service does not have yet.
    pub async fn pay_out(&self, payout_id: &Uuid) -> MyResult<()> {
        // A payout we have never heard of means our own commit has not landed, which
        // cannot happen — the event is enqueued in the same transaction as the row.
        // Erroring rather than skipping, so if it ever does, the NAK finds it.
        let payout = PayoutRepository::find_by_id(&self.db, *payout_id)
            .await?
            .ok_or_else(|| MyError::Bus(format!("payout {payout_id} not written; retrying")))?;

        if payout.status != status::REQUESTED {
            return Ok(());
        }

        // Onboarding is checked at the request, but between then and here a host could
        // in principle have no account: the row is this service's, the check was a
        // Stripe call, and the two are not one transaction. Failing rather than
        // silently marking the payout failed — a NAK retries, and a host who genuinely
        // has no account is a bug worth seeing in the log rather than a withdrawal
        // that quietly evaporates.
        let account_id = ConnectAccountRepository::find(&self.db, &payout.owner_id)
            .await?
            .ok_or_else(|| {
                MyError::Bus(format!(
                    "payout {payout_id} is for host {} with no connected account",
                    payout.owner_id
                ))
            })?;

        let owner_id = payout.owner_id;
        let (patch, event) = match self
            .stripe
            .transfer(&account_id, payout.amount_cents, payout_id)
            .await?
        {
            Transferred::Ok(transfer_id) => {
                tracing::info!(%payout_id, %transfer_id, "transferred");
                (
                    PayoutPatch::paid(transfer_id.clone()),
                    PaymentEvent::PayoutPaid {
                        payout_id: *payout_id,
                        owner_id,
                        transfer_id,
                        // Read once, here, and used for the row and the event alike —
                        // same rule as `Refunded::refunded_at`.
                        paid_at: Utc::now(),
                    },
                )
            }

            // Stripe's own refusal, and it will be the same refusal every time. The
            // money comes back by this row leaving `total_for`'s sum; there is nothing
            // to credit.
            Transferred::Refused(reason) => {
                tracing::warn!(%payout_id, %reason, "payout failed");
                (
                    PayoutPatch::failed(reason.clone()),
                    PaymentEvent::PayoutFailed {
                        payout_id: *payout_id,
                        owner_id,
                        reason,
                        failed_at: Utc::now(),
                    },
                )
            }
        };

        let mut tx = self.db.begin().await?;
        let version = db::next_version(&mut tx, "payout", payout_id).await?;

        // The row moves in the same transaction as the event, guarded on the status the
        // decision was made from.
        PayoutRepository::transition(&mut *tx, *payout_id, &[status::REQUESTED], patch).await?;
        db::set_version(&mut tx, "payout", payout_id, version).await?;

        // Deterministic event id: a redelivery that gets this far — because Stripe
        // answered but the commit did not — is discarded by the stream's duplicate
        // window rather than recorded twice.
        let mut envelope = Envelope::new(
            event,
            Some(owner_id),
            aggregate_id("payout", payout_id),
            version,
        );
        envelope.event_id = Uuid::new_v5(&Uuid::NAMESPACE_OID, format!("payout:{payout_id}").as_bytes());

        // The same subject as the request, so one payout's events stay in order.
        outbox::enqueue(&mut *tx, &payout_subject(&owner_id), &envelope).await?;
        tx.commit().await?;

        Ok(())
    }
}
