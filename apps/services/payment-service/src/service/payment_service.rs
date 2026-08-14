//! Write side. Validates and publishes — it never writes to the database.
//!
//! Every payment row this service reads back was put there by its own projector
//! applying an event this file published. See the note on `bus::publish`.

use std::sync::Arc;

use async_nats::jetstream::Context;
use chrono::Utc;
use shared::{
    error::myerror::{ContextExt, MyError, MyResult},
    events::{
        Envelope, STREAM_PAYMENTS,
        payment::{PaymentCreated, PaymentEvent},
        payment_subject, payout_subject, shard_of,
        user::record_key,
    },
};
use uuid::Uuid;

use axum::http::StatusCode;

use crate::{
    repository::payment_repository::{Earnings, PaymentRepository},
    service::{
        settle::Settler,
        stripe::{NewSession, Outcome, SessionState, Stripe},
    },
};

pub struct PaymentService {
    js: Context,
    repository: Arc<PaymentRepository>,
    stripe: Arc<Stripe>,
    settler: Arc<Settler>,
    /// How long after a booking ends its money becomes withdrawable.
    settlement_secs: i64,
}

impl PaymentService {
    pub fn new(
        js: Context,
        repository: Arc<PaymentRepository>,
        stripe: Arc<Stripe>,
        settler: Arc<Settler>,
        settlement_secs: i64,
    ) -> Self {
        Self {
            js,
            repository,
            stripe,
            settler,
            settlement_secs,
        }
    }

    /// Hands the renter a client secret for the Payment Element.
    ///
    /// Fully idempotent, with no read-and-branch: the payment id is derived from the
    /// booking, and so is the Stripe idempotency key. Calling this twice returns the
    /// *same* session from Stripe and upserts the same row here — which matters because
    /// the row does not exist yet when the request returns (the projector applies the
    /// event a moment later), so a double-submitted checkout has nothing to read to
    /// discover it is a duplicate.
    ///
    /// That property is also what makes `payment_booking UNIQUE` in the schema safe.
    pub async fn create_session(
        &self,
        booking_id: &str,
        renter_id: &str,
        return_url: &str,
    ) -> MyResult<NewSession> {
        let booking_uuid = parse_uuid(booking_id)?;
        let key = record_key(&booking_uuid);

        let booking = self
            .repository
            .booking(&key)
            .await?
            .context_not_found(("Not Found", "That booking doesn't exist."))?;

        // Identity comes from the verified token, and this is the only check that
        // stops one renter paying for — and thereby confirming — another's booking.
        if booking.renter_id != renter_id {
            return Err(MyError::api(
                axum::http::StatusCode::FORBIDDEN,
                "Forbidden",
                "That booking isn't yours.",
            ));
        }

        if booking.status != "reserved" {
            return Err(MyError::api(
                axum::http::StatusCode::CONFLICT,
                "Conflict",
                "That booking is no longer awaiting payment.",
            ));
        }

        // An expired hold must not be payable. The sweeper may not have collected it yet,
        // so the timestamp is the authority here, not the status.
        //
        // Kept rather than just checked: it anchors the session's `expires_at`, which has
        // to be identical on every call for this booking or the idempotent replay behind
        // "Continue payment" is rejected. See SESSION_GRACE_MINUTES.
        let hold_until = booking
            .hold_until
            .filter(|until| *until > Utc::now())
            .ok_or_else(|| {
                MyError::api(
                    axum::http::StatusCode::GONE,
                    "Hold Expired",
                    "This reservation has expired. Please choose your times again.",
                )
            })?;

        let payment_id = payment_id_for(&booking_uuid);
        let session = self
            .stripe
            .create_session(
                &booking_uuid,
                booking.amount_cents,
                &booking.booked,
                hold_until,
                return_url,
            )
            .await?;

        let event = PaymentEvent::Created(PaymentCreated {
            payment_id,
            booking_id: booking_uuid,
            booking_shard: shard_of(&booking_uuid),
            owner_id: booking.owner_id,
            renter_id: booking.renter_id,
            session_id: session.session_id.clone(),
            amount_cents: booking.amount_cents,
            created_at: Utc::now(),
        });

        self.publish_payment(event, &booking_uuid, &format!("payment-created:{payment_id}"))
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
        session_id: &str,
        renter_id: &str,
    ) -> MyResult<(SessionState, String)> {
        let not_found = || MyError::api(StatusCode::NOT_FOUND, "Not Found", "No such checkout.");

        let payment = self
            .repository
            .payment_for_session(session_id)
            .await?
            .ok_or_else(not_found)?;

        if payment.renter_id != renter_id {
            return Err(not_found());
        }

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
                let payment_id = payment_id_for(&booking_id);
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
                let payment_id = payment_id_for(&booking_id);
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

        self.publish_payment(event, &booking_id, &dedupe).await?;
        Ok(())
    }

    pub async fn earnings(&self, owner_id: &str) -> MyResult<Earnings> {
        let cutoff = Utc::now() - chrono::Duration::seconds(self.settlement_secs);
        self.repository.earnings(owner_id, cutoff).await
    }

    /// "Take the money out." Nothing leaves any real account.
    ///
    /// The amount is computed here and a client-supplied figure is never read. Two
    /// concurrent requests both see the same affordable balance, which is why this
    /// publishes under compare-and-swap on the host's own subject: exactly one wins and
    /// the loser gets a 409 to retry against the reduced balance.
    pub async fn request_payout(&self, owner_id: &str) -> MyResult<(u64, i64)> {
        let amount_cents = self.settler.available_for(owner_id, self.settlement_secs).await?;

        if amount_cents <= 0 {
            return Err(MyError::api(
                axum::http::StatusCode::UNPROCESSABLE_ENTITY,
                "Nothing to Withdraw",
                "You have no settled earnings yet.",
            ));
        }

        let owner_uuid = owner_uuid(owner_id)?;
        let shard = shard_of(&owner_uuid);
        let subject = payout_subject(&shard, &owner_uuid);
        let head = bus::subject_head(&self.js, STREAM_PAYMENTS, &subject).await?;

        let envelope = Envelope::new(
            PaymentEvent::PayoutRequested {
                payout_id: Uuid::now_v7(),
                owner_id: owner_id.to_string(),
                amount_cents,
                requested_at: Utc::now(),
            },
            Some(owner_id.to_string()),
        );

        let seq = bus::publish_expecting(&self.js, subject, &envelope, Some(head)).await?;
        Ok((seq, amount_cents))
    }

    /// Publishes onto a booking's payment subject with a deterministic event id.
    ///
    /// The id is what makes a retried publish — a redelivered webhook, a resubmitted
    /// checkout — discarded by the stream's duplicate window instead of appended twice.
    /// No compare-and-swap: one payment owns this subject, so there is no second writer
    /// to lose a race to.
    async fn publish_payment(
        &self,
        event: PaymentEvent,
        booking_id: &Uuid,
        dedupe: &str,
    ) -> MyResult<u64> {
        let mut envelope = Envelope::new(event, None);
        envelope.event_id = Uuid::new_v5(&Uuid::NAMESPACE_OID, dedupe.as_bytes());

        Ok(bus::publish(
            &self.js,
            payment_subject(&shard_of(booking_id), booking_id),
            &envelope,
        )
        .await?)
    }
}

/// One payment per booking, named after it.
///
/// Deterministic rather than random so that creating an intent is idempotent without a
/// read: the second attempt upserts the same row instead of tripping the UNIQUE index
/// on `booking_id`, and a webhook can name the payment without looking it up.
pub fn payment_id_for(booking_id: &Uuid) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_OID,
        format!("payment:{booking_id}").as_bytes(),
    )
}

/// `"user:019fafc9…"` -> the uuid, for sharding.
fn owner_uuid(owner_id: &str) -> MyResult<Uuid> {
    let key = owner_id.strip_prefix("user:").unwrap_or(owner_id);
    Uuid::parse_str(key).map_err(|e| MyError::Bus(format!("owner id {owner_id}: {e}")))
}

fn parse_uuid(id: &str) -> MyResult<Uuid> {
    Uuid::parse_str(id).map_err(|_| {
        MyError::api(
            axum::http::StatusCode::BAD_REQUEST,
            "Bad Request",
            "That isn't a valid booking id.",
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The property the UNIQUE index and the webhook lookup both depend on.
    #[test]
    fn a_booking_always_maps_to_the_same_payment_id() {
        let booking = Uuid::now_v7();
        assert_eq!(payment_id_for(&booking), payment_id_for(&booking));
        assert_ne!(payment_id_for(&booking), payment_id_for(&Uuid::now_v7()));
    }

    #[test]
    fn owner_uuid_accepts_the_claim_form_and_the_bare_key() {
        let id = Uuid::now_v7();
        let key = record_key(&id);
        assert_eq!(owner_uuid(&format!("user:{key}")).unwrap(), id);
        assert_eq!(owner_uuid(&key).unwrap(), id);
        assert!(owner_uuid("user:not-a-uuid").is_err());
    }
}
