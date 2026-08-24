//! Write side. Validates and publishes — it never writes to the database.
//!
//! Every payment row this service reads back was put there by its own projector
//! applying an event this file published. See the note on `bus::publish`.

use std::sync::Arc;

use async_nats::jetstream::Context;
use chrono::Utc;
use shared::{
    domain_models::{booking::status as booking_status, payment::Earnings},
    error::myerror::{ContextExt, MyResult},
    events::{
        Envelope, STREAM_PAYMENTS,
        payment::{PaymentCreated, PaymentEvent},
        payment_subject, payout_subject, shard_of,
    },
    requests::payment::CreateSessionRequest,
    rpc::spot::{SUBJECT_SPOT_CARD, SpotCard},
};
use surrealdb::{Surreal, engine::remote::ws::Client};
use uuid::Uuid;

use axum::http::StatusCode;

use crate::{
    client::stripe::{NewSession, Outcome, SessionState, Stripe},
    repository::{
        booking_mirror_repository::BookingMirrorRepository, payment_repository::PaymentRepository,
        payout_repository::PayoutRepository,
    },
};

pub struct PaymentService {
    js: Context,
    /// The same connection `js` publishes over, for the one thing JetStream is wrong for:
    /// asking spot-service what a spot is called. See [`shared::rpc`].
    nc: async_nats::Client,
    payments: PaymentRepository,
    bookings: BookingMirrorRepository,
    /// Read-only, and only for `earnings` — payout *rows* are written by this service's
    /// projector, never here.
    payouts: PayoutRepository,
    stripe: Arc<Stripe>,
    /// How long after a booking ends its money becomes withdrawable.
    settlement_secs: i64,
}

impl PaymentService {
    pub fn new(
        js: Context,
        db: Arc<Surreal<Client>>,
        stripe: Arc<Stripe>,
        settlement_secs: i64,
    ) -> Self {
        Self {
            nc: js.client().clone(),
            js,
            payments: PaymentRepository { q: db.clone() },
            bookings: BookingMirrorRepository { q: db.clone() },
            payouts: PayoutRepository { q: db },
            stripe,
            settlement_secs,
        }
    }

    /// Hands the renter a client secret for the Payment Element.
    ///
    /// Idempotent twice over, and the two layers are not redundant:
    ///
    /// 1. **A payment we already know about is never re-created.** "Continue payment" on a
    ///    reserved booking retrieves the session it already has. Nothing is rebuilt, so
    ///    nothing can be rebuilt *differently* — which matters because the Stripe
    ///    idempotency key is derived from the booking, and Stripe rejects a replay whose
    ///    parameters differ. A spot retitled between reserving and paying would otherwise
    ///    fail the resume, the same way an `expires_at` taken from `Utc::now()` once did.
    /// 2. **Below that, the key still holds.** Two checkouts submitted at once both find
    ///    no payment row — the projector applies our event a moment after this returns —
    ///    so both reach Stripe, and the key is what makes them the same session rather
    ///    than two payable ones.
    ///
    /// Layer 2 is also what makes `payment_booking UNIQUE` in the schema safe.
    pub async fn create_session(
        &self,
        renter_id: &Uuid,
        request: CreateSessionRequest,
    ) -> MyResult<NewSession> {
        let booking_id = &request.booking_id;
        let booking = self
            .bookings
            .find_by_id(*booking_id)
            .await?
            .context_not_found(("Not Found", "That booking doesn't exist."))?;

        // Identity comes from the verified token, and this is the only check that
        // stops one renter paying for — and thereby confirming — another's booking.
        (booking.renter_id == *renter_id)
            .context_forbidden(("Forbidden", "That booking isn't yours."))?;

        (booking.status == booking_status::RESERVED)
            .context_conflict(("Conflict", "That booking is no longer awaiting payment."))?;

        // An expired hold must not be payable. The sweeper may not have collected it yet,
        // so the timestamp is the authority here, not the status.
        //
        // Kept rather than just checked: it anchors the session's `expires_at`, which has
        // to be identical on every call for this booking or the idempotent replay behind
        // "Continue payment" is rejected. See SESSION_GRACE_MINUTES.
        let hold_until = booking
            .hold_until
            .filter(|until| *until > Utc::now())
            .context_status(
                StatusCode::GONE,
                (
                    "Hold Expired",
                    "This reservation has expired. Please choose your times again.",
                ),
            )?;

        // Resume: this booking already has a session, so hand back that one. See the
        // note on layer 1 above — re-creating it is not merely wasteful, it is the thing
        // that breaks. A session that has since expired falls through, where the guards
        // above have already refused anything whose hold is gone.
        if let Some(payment) = self.payments.find_by_booking_id(*booking_id).await? {
            let state = self.stripe.retrieve_session(&payment.session_id).await?;
            if let Some(client_secret) = state.client_secret {
                return Ok(NewSession {
                    session_id: payment.session_id,
                    client_secret,
                });
            }
        }

        // What the renter is buying, for the line item. Best-effort on purpose: a spot
        // this cannot describe still gets paid for, it just says less. Nothing below is
        // allowed to depend on it — see the note on `shared::rpc`.
        let spot_id = booking.spot_id;
        let card: Option<SpotCard> = bus::service::request(&self.nc, SUBJECT_SPOT_CARD, &spot_id)
            .await
            .unwrap_or_else(|e| {
                tracing::warn!(spot_id = %spot_id, error = %e, "no spot card; labelling with the booking id");
                None
            });

        // One payment per booking, named after it. Deterministic rather than random so
        // that creating an intent is idempotent without a read: the second attempt
        // upserts the same row instead of tripping the UNIQUE index on `booking_id`, and
        // a webhook can name the payment without looking it up.
        let payment_id = Uuid::new_v5(
            &Uuid::NAMESPACE_OID,
            format!("payment:{booking_id}").as_bytes(),
        );

        let session = self
            .stripe
            .create_session(
                booking_id,
                booking.amount_cents,
                &booking.booked,
                card.as_ref(),
                hold_until,
                &request.return_url,
            )
            .await?;

        let event = PaymentEvent::Created(PaymentCreated {
            payment_id,
            booking_id: *booking_id,
            booking_shard: shard_of(booking_id),
            owner_id: booking.owner_id,
            renter_id: booking.renter_id,
            session_id: session.session_id.clone(),
            amount_cents: booking.amount_cents,
            created_at: Utc::now(),
        });

        // Deterministic event id, so a resubmitted checkout is discarded by the stream's
        // duplicate window instead of appended twice. No compare-and-swap: one payment
        // owns this subject, so there is no second writer to lose a race to.
        let mut envelope = Envelope::new(event, None);
        envelope.event_id = Uuid::new_v5(
            &Uuid::NAMESPACE_OID,
            format!("payment-created:{payment_id}").as_bytes(),
        );

        bus::publish(
            &self.js,
            payment_subject(&shard_of(booking_id), booking_id),
            &envelope,
        )
        .await?;

        Ok(session)
    }

    /// What became of a session, for the checkout screen.
    ///
    /// Authorized against our own `payment` row before Stripe is asked anything, so a
    /// session id belonging to someone else reveals nothing. **404 rather than 403**: a
    /// 403 would confirm the session exists to a caller who cannot see it.
    ///
    /// The status itself comes from Stripe rather than our projection, because our
    /// projection lags the webhook and "not confirmed yet" is indistinguishable from
    /// "failed" from the outside. Stripe knows now.
    pub async fn session_state(
        &self,
        renter_id: &Uuid,
        session_id: &str,
    ) -> MyResult<(SessionState, Uuid)> {
        // The same answer for a session that does not exist and one that is not the
        // caller's — a 403 would confirm it exists to someone who cannot see it.
        const NOT_FOUND: (&str, &str) = ("Not Found", "No such checkout.");

        let payment = self
            .payments
            .find_by_session_id(session_id.to_string())
            .await?
            .context_not_found(NOT_FOUND)?;

        (payment.renter_id == *renter_id).context_not_found(NOT_FOUND)?;

        let state = self.stripe.retrieve_session(session_id).await?;
        Ok((state, payment.booking_id))
    }

    /// Records what a verified webhook said.
    ///
    /// Publishes unconditionally for `Succeeded`. Money has already moved by the time
    /// Stripe tells us, so there is nothing to refuse — a booking that no longer fits
    /// is withdrawn downstream and refunded by `settle_up`, which is one refund trigger
    /// rather than a decision made here with half the information.
    pub async fn record_webhook(&self, outcome: Outcome) -> MyResult<()> {
        let (event, booking_id, dedupe) = match outcome {
            Outcome::Succeeded {
                booking_id,
                intent_id,
            } => {
                // Same derivation as `create_session` — that is the point: the webhook
                // names the payment without reading it back.
                let payment_id = Uuid::new_v5(
                    &Uuid::NAMESPACE_OID,
                    format!("payment:{booking_id}").as_bytes(),
                );
                (
                    PaymentEvent::Succeeded {
                        payment_id,
                        booking_id,
                        intent_id,
                    },
                    booking_id,
                    format!("payment-succeeded:{payment_id}"),
                )
            }

            Outcome::Failed { booking_id, reason } => {
                let payment_id = Uuid::new_v5(
                    &Uuid::NAMESPACE_OID,
                    format!("payment:{booking_id}").as_bytes(),
                );
                (
                    // Keyed on the reason as well as the payment: a renter retrying a
                    // declined card twice produces two genuine failures, and swallowing
                    // the second as a duplicate would lose it.
                    PaymentEvent::Failed {
                        payment_id,
                        booking_id,
                        reason: reason.clone(),
                    },
                    booking_id,
                    format!("payment-failed:{payment_id}:{reason}"),
                )
            }

            // Not ours, or a type we don't handle. The caller still answers 200.
            Outcome::Ignored => return Ok(()),
        };

        // Same as above: the id is what makes a redelivered webhook a no-op.
        let mut envelope = Envelope::new(event, None);
        envelope.event_id = Uuid::new_v5(&Uuid::NAMESPACE_OID, dedupe.as_bytes());

        bus::publish(
            &self.js,
            payment_subject(&shard_of(&booking_id), &booking_id),
            &envelope,
        )
        .await?;
        Ok(())
    }

    /// A host's settled income and what they have already taken out.
    ///
    /// Two queries over two tables rather than one join: each repository answers for
    /// its own table, and the subtraction is arithmetic that belongs here.
    pub async fn earnings(&self, owner_id: &Uuid) -> MyResult<Earnings> {
        let cutoff = Utc::now() - chrono::Duration::seconds(self.settlement_secs);
        Ok(Earnings {
            earned_cents: self.payments.earned(owner_id, cutoff).await?,
            paid_out_cents: self.payouts.total_for(owner_id).await?,
        })
    }

    /// Whether a host may withdraw, and how much. Derived, never stored.
    async fn available_for(&self, owner_id: &Uuid) -> MyResult<i64> {
        Ok(self.earnings(owner_id).await?.available_cents())
    }

    /// "Take the money out." Nothing leaves any real account.
    ///
    /// The amount is computed here and a client-supplied figure is never read. Two
    /// concurrent requests both see the same affordable balance, which is why this
    /// publishes under compare-and-swap on the host's own subject: exactly one wins and
    /// the loser gets a 409 to retry against the reduced balance.
    pub async fn request_payout(&self, owner_id: &Uuid) -> MyResult<(u64, i64)> {
        let amount_cents = self.available_for(owner_id).await?;

        (amount_cents > 0).context_unprocessable_entity((
            "Nothing to Withdraw",
            "You have no settled earnings yet.",
        ))?;

        let shard = shard_of(owner_id);
        let subject = payout_subject(&shard, owner_id);
        let head = bus::subject_head(&self.js, STREAM_PAYMENTS, &subject).await?;

        let envelope = Envelope::new(
            PaymentEvent::PayoutRequested {
                payout_id: Uuid::now_v7(),
                owner_id: *owner_id,
                amount_cents,
                requested_at: Utc::now(),
            },
            Some(*owner_id),
        );

        let seq = bus::publish_expecting(&self.js, subject, &envelope, Some(head)).await?;
        Ok((seq, amount_cents))
    }
}
