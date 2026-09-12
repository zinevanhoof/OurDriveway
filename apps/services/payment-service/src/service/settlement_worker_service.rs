//! Carrying out what happens to money when a booking ends.
//!
//! One rule, reached from two directions — but only ever from a `Worker`. A booking can
//! end before its payment resolves, and a payment can resolve after its booking has
//! ended, so both `BookingWorker` and `PaymentWorker` call
//! [`SettlementWorkerService::settle_up`] and it decides from current state rather than
//! from which event woke it. That is also why the type is not named for either stream:
//! naming it after one of two callers would hide the other.
//!
//! The rule itself is [`crate::policy::settlement::decide`] — a pure function over two
//! statuses. What is left here is the I/O around it: reading the projections it decides
//! from, calling Stripe, publishing the outcome.
//!
//! Reading a host's balance used to live here too. It moved to `PaymentService`, where
//! its actual callers are — two routes — so that everything left in this file is
//! worker-driven and the name stays true.

use bus::outbox;
use chrono::Utc;
use diesel_async::AsyncConnection;
use diesel_async::scoped_futures::ScopedFutureExt;
use shared::db;
use shared::{
    domain_models::payment::{PaymentPatch, status},
    error::myerror::{MyError, MyResult},
    events::{Envelope, aggregate_id, payment::PaymentEvent, payment_subject},
};
use std::sync::Arc;
use uuid::Uuid;

use crate::{
    client::stripe::Stripe,
    policy::settlement::{Action, decide},
    repository::{
        booking_mirror_repository::BookingMirrorRepository, payment_repository::PaymentRepository,
    },
};

pub struct SettlementWorkerService {
    /// The pool. See the note on `UserService::db` — the repositories are stateless.
    pub db: shared::db::Db,
    pub stripe: Arc<Stripe>,
}

impl SettlementWorkerService {
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
        // A payment we have never heard of is the common case, not a problem: most
        // bookings are released without anyone reaching checkout.
        let mut read = db::conn(&self.db).await?;
        let Some(payment) = PaymentRepository::find_by_booking_id(&mut read, *booking_id).await?
        else {
            return Ok(());
        };

        // The booking, on the other hand, must exist — this payment was created from
        // it. Missing means our own BOOKINGS projection is behind, so fail and let the
        // redelivery find it rather than silently skipping a refund.
        let mut read = db::conn(&self.db).await?;
        let booking = BookingMirrorRepository::find_by_id(&mut read, *booking_id)
            .await?
            .ok_or_else(|| {
                MyError::Bus(format!("booking {booking_id} not projected yet; retrying"))
            })?;

        let payment_id = payment.id;

        let event = match decide(
            &booking.status,
            &payment.status,
            payment.refund_id.is_some(),
        ) {
            Action::Nothing => return Ok(()),

            Action::Refund => {
                // Non-null by construction: `intent_id` is written in the same statement
                // as `status = 'succeeded'`, and that status is the only one this arm
                // matches. Erroring rather than unwrapping so a future change that breaks
                // the pairing surfaces as a retry, not a panic in a money path.
                let intent_id = payment.intent_id.as_deref().ok_or_else(|| {
                    MyError::Bus(format!(
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
                    // Read once, here, and used for both the row and the event below —
                    // not read again at either write. A second `Utc::now()` would give
                    // this service's table and view-service's a different answer for
                    // the same refund.
                    refunded_at: Utc::now(),
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

        let mut conn = db::conn(&self.db).await?;

        conn.transaction::<_, MyError, _>(|conn| {
            async move {
                let version = shared::next_version!(conn, shared::schema::payment::payment, &payment_id)?;

                // The row moves in the same transaction as the event. Guarded, so a
                // redelivery that already applied is a no-op rather than a second refund.
                match &event {
                    PaymentEvent::Refunded {
                        refund_id,
                        refunded_at,
                        ..
                    } => {
                        PaymentRepository::transition(
                            conn,
                            payment_id,
                            &[status::SUCCEEDED],
                            PaymentPatch::refunded(refund_id.clone(), *refunded_at),
                        )
                        .await?;
                    }
                    PaymentEvent::SessionExpired { .. } => {
                        PaymentRepository::transition(
                            conn,
                            payment_id,
                            &status::UNPAID,
                            PaymentPatch::expired(),
                        )
                        .await?;
                    }
                    // `decide` yields only the two above.
                    _ => {}
                }
                shared::set_version!(conn, "payment", shared::schema::payment::payment, &payment_id, version)?;

                // Deterministic event id: a redelivery that gets this far — because the Stripe
                // call succeeded but the commit did not — is discarded by the stream's
                // duplicate window rather than recorded twice.
                let mut envelope =
                    Envelope::new(event, None, aggregate_id("payment", &payment_id), version);
                envelope.event_id = Uuid::new_v5(
                    &Uuid::NAMESPACE_OID,
                    format!("settle:{payment_id}:{}", booking.status).as_bytes(),
                );

                outbox::enqueue(conn, &payment_subject(booking_id), &envelope).await?;
                Ok(())
            }
            .scope_boxed()
        })
        .await?;

        Ok(())
    }
}
