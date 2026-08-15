use chrono::{DateTime, Utc};
use serde::Deserialize;
use shared::{
    error::myerror::MyResult,
    events::{
        Envelope,
        booking::{BookingEvent, BookingReserved, CancelReason, ReleaseReason},
        payment::{PaymentCreated, PaymentEvent},
    },
    general_models::booking::Booked,
};
use surrealdb::{
    Surreal,
    engine::remote::ws::Client,
    types::{Datetime, SurrealValue},
};
use uuid::Uuid;

/// The **only** writer to this database. Request handlers publish to NATS and
/// return; everything that lands here arrives via a projector.
pub struct PaymentRepository {
    pub db: Surreal<Client>,
}

/// A booking as this service sees it, which is everything needed to decide whether a
/// payment may be created and what happens to money afterwards.
#[derive(Debug, Deserialize, SurrealValue)]
pub struct BookingForPayment {
    /// Which spot, so the checkout can ask spot-service what to call it. The id alone —
    /// payment-service holds no spot data and consumes no SPOTS events. See
    /// [`shared::rpc::spot`].
    pub spot_id: Uuid,
    pub owner_id: Uuid,
    pub renter_id: Uuid,
    pub amount_cents: i64,
    pub status: String,
    pub hold_until: Option<DateTime<Utc>>,
    pub ends_at: DateTime<Utc>,
    /// Wall-clock slots in the spot's zone. Read only to describe the purchase on the
    /// Checkout Session's line item — see the field comment in payment-schema.surql.
    pub booked: Booked,
}

/// The payment attached to one booking, if any.
#[derive(Debug, Deserialize, SurrealValue)]
pub struct PaymentRow {
    /// The uuid straight from `record::id(id)` — it goes back into an event as-is.
    pub id: Uuid,
    pub booking_id: Uuid,
    /// Where this payment's events publish. Read back rather than recomputed — see
    /// the field comment in payment-schema.surql.
    pub booking_shard: String,
    /// `cs_…`, always present. Expires an unpaid checkout.
    pub session_id: String,
    /// `pi_…`, NONE until the payment succeeded. Refunds need this; a session is not
    /// accepted by the refund API.
    pub intent_id: Option<String>,
    pub amount_cents: i64,
    pub status: String,
    pub refund_id: Option<String>,
    /// Who paid. Read so the session lookup can scope its answer to the asking renter.
    pub renter_id: Uuid,
    /// Stripe's message from the last failed attempt, carried alongside `status =
    /// 'failed'` so the checkout screen can say *why* rather than just that it failed.
    pub failure_reason: Option<String>,
}

/// A host's money, in one query. All three are derived; none is stored.
#[derive(Debug, Default, Deserialize, SurrealValue)]
pub struct Earnings {
    /// Settled income: paid, still confirmed, and past the settlement window.
    pub earned_cents: i64,
    pub paid_out_cents: i64,
}

impl Earnings {
    pub fn available_cents(&self) -> i64 {
        self.earned_cents - self.paid_out_cents
    }
}

impl PaymentRepository {
    pub async fn last_seq(&self, stream: &str) -> MyResult<u64> {
        let seq: Option<i64> = self
            .db
            .query("SELECT VALUE last_seq FROM ONLY type::record('_projection', $s)")
            .bind(("s", stream.to_string()))
            .await?
            .take(0)?;
        Ok(seq.unwrap_or(0).max(0) as u64)
    }

    // ─── reads ──────────────────────────────────────────────────────────────

    pub async fn booking(&self, booking_id: &Uuid) -> MyResult<Option<BookingForPayment>> {
        Ok(self
            .db
            .query(
                "SELECT spot_id, owner_id, renter_id, amount_cents, status, hold_until,
                        ends_at, booked ?? {} AS booked
                 FROM ONLY type::record('booking', $id)",
            )
            .bind(("id", *booking_id))
            .await?
            .take(0)?)
    }

    /// The payment for a booking. `payment_booking` is UNIQUE, so this is at most one.
    pub async fn payment_for_booking(&self, booking_id: &Uuid) -> MyResult<Option<PaymentRow>> {
        Ok(self
            .db
            .query(
                "SELECT record::id(id) AS id, booking_id, booking_shard, session_id,
                        intent_id, amount_cents, status, refund_id, renter_id,
                        failure_reason
                 FROM ONLY payment WHERE booking_id = $b LIMIT 1",
            )
            .bind(("b", booking_id.to_string()))
            .await?
            .take(0)?)
    }

    /// The payment behind a Checkout Session. `payment_session` is UNIQUE, so at most one.
    ///
    /// This is what turns the session id in a checkout URL into something we can
    /// authorize: the row carries `renter_id`, so the handler can refuse a session that
    /// isn't the caller's *before* asking Stripe anything about it.
    pub async fn payment_for_session(&self, session_id: &str) -> MyResult<Option<PaymentRow>> {
        Ok(self
            .db
            .query(
                "SELECT record::id(id) AS id, booking_id, booking_shard, session_id,
                        intent_id, amount_cents, status, refund_id, renter_id,
                        failure_reason
                 FROM ONLY payment WHERE session_id = $s LIMIT 1",
            )
            .bind(("s", session_id.to_string()))
            .await?
            .take(0)?)
    }

    /// A host's settled income and what they have already withdrawn.
    ///
    /// Two conditions, and both are needed. The payment must have succeeded and not
    /// been refunded; the *booking* must still be confirmed and old enough to have
    /// settled. The booking half is what stops a host withdrawing money for a booking
    /// that has not happened yet — see SETTLEMENT_SECS in `Config`.
    ///
    /// `cutoff` is passed in rather than read from a clock here so the caller decides
    /// the window and this stays a pure query.
    pub async fn earnings(&self, owner_id: &Uuid, cutoff: DateTime<Utc>) -> MyResult<Earnings> {
        let earned: Option<i64> = self
            .db
            .query(
                "SELECT VALUE math::sum(amount_cents) FROM ONLY (
                     SELECT amount_cents FROM payment
                     WHERE owner_id = $o AND status = 'succeeded'
                       AND booking_id IN (
                           SELECT VALUE record::id(id) FROM booking
                           WHERE owner_id = $o AND status = 'confirmed' AND ends_at < $cutoff
                       )
                 ) GROUP ALL",
            )
            .bind(("o", *owner_id))
            .bind(("cutoff", Datetime::from(cutoff)))
            .await?
            .take(0)?;

        let paid_out: Option<i64> = self
            .db
            .query(
                "SELECT VALUE math::sum(amount_cents) FROM ONLY (
                     SELECT amount_cents FROM payout WHERE owner_id = $o
                 ) GROUP ALL",
            )
            .bind(("o", *owner_id))
            .await?
            .take(0)?;

        Ok(Earnings {
            earned_cents: earned.unwrap_or(0),
            paid_out_cents: paid_out.unwrap_or(0),
        })
    }

    // ─── BOOKINGS projection ────────────────────────────────────────────────

    pub async fn apply_booking(&self, envelope: Envelope<BookingEvent>, seq: u64) -> MyResult<()> {
        let at = envelope.occurred_at;
        match envelope.payload {
            BookingEvent::Reserved(e) => self.reserved(e, at, seq).await,
            BookingEvent::Confirmed { booking_id } => {
                self.booking_status(booking_id, "confirmed", &["reserved"], at, seq)
                    .await
            }
            BookingEvent::Released { booking_id, reason } => {
                self.released(booking_id, reason, at, seq).await
            }
            BookingEvent::Cancelled { booking_id, reason } => {
                self.cancelled(booking_id, reason, at, seq).await
            }
        }
    }

    async fn reserved(&self, e: BookingReserved, at: DateTime<Utc>, seq: u64) -> MyResult<()> {
        self.db
            .query(
                "BEGIN;
                 UPSERT type::record('booking', $id) CONTENT {
                     spot_id: $spot_id, spot_shard: $spot_shard, owner_id: $owner_id,
                     renter_id: $renter_id, amount_cents: $amount, status: 'reserved',
                     booked: $booked, hold_until: $expires_at, ends_at: $ends_at
                 };
                 UPSERT _projection:BOOKINGS SET last_seq = $seq, updated_at = $at;
                 COMMIT;",
            )
            .bind(("id", e.booking_id))
            .bind(("spot_id", e.spot_id))
            .bind(("spot_shard", e.spot_shard))
            .bind(("owner_id", e.owner_id))
            .bind(("renter_id", e.renter_id))
            .bind(("amount", e.amount_cents))
            // Only ever read to describe the purchase on the Stripe line item.
            .bind(("booked", e.booked))
            .bind(("expires_at", Datetime::from(e.expires_at)))
            .bind(("ends_at", Datetime::from(e.ends_at)))
            .bind(("seq", seq as i64))
            .bind(("at", Datetime::from(at)))
            .await?
            .check()?;
        Ok(())
    }

    async fn released(
        &self,
        booking_id: Uuid,
        reason: ReleaseReason,
        at: DateTime<Utc>,
        seq: u64,
    ) -> MyResult<()> {
        // Scoped to 'reserved' for the same reason booking-service's own projector
        // scopes it: a payment landing microseconds before the hold lapses, with the
        // sweeper's event arriving second, must not undo the confirmation.
        self.db
            .query(
                "BEGIN;
                 UPDATE type::record('booking', $id)
                     SET status = 'released', release_reason = $reason, hold_until = NONE
                     WHERE status = 'reserved';
                 UPSERT _projection:BOOKINGS SET last_seq = $seq, updated_at = $at;
                 COMMIT;",
            )
            .bind(("id", booking_id))
            .bind(("reason", reason.as_str().to_string()))
            .bind(("seq", seq as i64))
            .bind(("at", Datetime::from(at)))
            .await?
            .check()?;
        Ok(())
    }

    async fn cancelled(
        &self,
        booking_id: Uuid,
        reason: CancelReason,
        at: DateTime<Utc>,
        seq: u64,
    ) -> MyResult<()> {
        self.db
            .query(
                "BEGIN;
                 UPDATE type::record('booking', $id)
                     SET status = 'cancelled', cancel_reason = $reason, hold_until = NONE
                     WHERE status = 'confirmed';
                 UPSERT _projection:BOOKINGS SET last_seq = $seq, updated_at = $at;
                 COMMIT;",
            )
            .bind(("id", booking_id))
            .bind(("reason", reason.as_str().to_string()))
            .bind(("seq", seq as i64))
            .bind(("at", Datetime::from(at)))
            .await?
            .check()?;
        Ok(())
    }

    async fn booking_status(
        &self,
        booking_id: Uuid,
        to: &str,
        from: &[&str],
        at: DateTime<Utc>,
        seq: u64,
    ) -> MyResult<()> {
        self.db
            .query(
                "BEGIN;
                 UPDATE type::record('booking', $id)
                     SET status = $to, hold_until = NONE
                     WHERE status IN $from;
                 UPSERT _projection:BOOKINGS SET last_seq = $seq, updated_at = $at;
                 COMMIT;",
            )
            .bind(("id", booking_id))
            .bind(("to", to.to_string()))
            .bind((
                "from",
                from.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            ))
            .bind(("seq", seq as i64))
            .bind(("at", Datetime::from(at)))
            .await?
            .check()?;
        Ok(())
    }

    // ─── PAYMENTS projection ────────────────────────────────────────────────

    pub async fn apply_payment(&self, envelope: Envelope<PaymentEvent>, seq: u64) -> MyResult<()> {
        let at = envelope.occurred_at;
        match envelope.payload {
            PaymentEvent::Created(e) => self.payment_created(e, at, seq).await,
            PaymentEvent::Succeeded {
                payment_id,
                intent_id,
                ..
            } => self.payment_succeeded(payment_id, intent_id, at, seq).await,
            PaymentEvent::Failed {
                payment_id, reason, ..
            } => self.payment_failed(payment_id, reason, at, seq).await,
            PaymentEvent::Refunded {
                payment_id,
                refund_id,
                ..
            } => {
                self.payment_status(payment_id, "refunded", Some(refund_id), at, seq)
                    .await
            }
            PaymentEvent::SessionExpired { payment_id, .. } => {
                self.payment_status(payment_id, "expired", None, at, seq)
                    .await
            }
            PaymentEvent::PayoutRequested {
                payout_id,
                owner_id,
                amount_cents,
                requested_at,
            } => {
                self.payout(payout_id, owner_id, amount_cents, requested_at, at, seq)
                    .await
            }
        }
    }

    async fn payment_created(
        &self,
        e: PaymentCreated,
        at: DateTime<Utc>,
        seq: u64,
    ) -> MyResult<()> {
        self.db
            .query(
                "BEGIN;
                 UPSERT type::record('payment', $id) CONTENT {
                     booking_id: $booking_id, booking_shard: $booking_shard,
                     owner_id: $owner_id, renter_id: $renter_id,
                     amount_cents: $amount, session_id: $session_id, status: 'created',
                     created_at: $created_at
                 };
                 UPSERT _projection:PAYMENTS SET last_seq = $seq, updated_at = $at;
                 COMMIT;",
            )
            .bind(("id", e.payment_id))
            .bind(("booking_id", e.booking_id))
            .bind(("booking_shard", e.booking_shard))
            .bind(("owner_id", e.owner_id))
            .bind(("renter_id", e.renter_id))
            .bind(("amount", e.amount_cents))
            .bind(("session_id", e.session_id))
            .bind(("created_at", Datetime::from(e.created_at)))
            .bind(("seq", seq as i64))
            .bind(("at", Datetime::from(at)))
            .await?
            .check()?;
        Ok(())
    }

    /// A failed attempt, recorded as its own status rather than inferred.
    ///
    /// `'failed'` is **not terminal**: the renter can confirm the same session again with
    /// another method, so `settle_up` still treats it exactly like `'created'` and expires
    /// the session if the booking ends unpaid. The status exists so that "tried and
    /// failed" is a fact the checkout screen can read, instead of something reconstructed
    /// from `'created'` plus a non-empty `failure_reason`.
    ///
    /// Reachable from `'created'` or from itself — two declines in a row are two genuine
    /// failures and the second must still record its reason.
    async fn payment_failed(
        &self,
        payment_id: Uuid,
        reason: String,
        at: DateTime<Utc>,
        seq: u64,
    ) -> MyResult<()> {
        self.db
            .query(
                "BEGIN;
                 UPDATE type::record('payment', $id)
                     SET status = 'failed', failure_reason = $reason
                     WHERE status IN ['created', 'failed'];
                 UPSERT _projection:PAYMENTS SET last_seq = $seq, updated_at = $at;
                 COMMIT;",
            )
            .bind(("id", payment_id))
            .bind(("reason", reason))
            .bind(("seq", seq as i64))
            .bind(("at", Datetime::from(at)))
            .await?
            .check()?;
        Ok(())
    }

    /// The payment succeeded, which is also where `intent_id` first becomes known.
    ///
    /// Both writes are in one statement on purpose. `settle_up`'s refund arm matches
    /// `status = 'succeeded'` and then needs `intent_id`, so a row that had one without
    /// the other would be a refund it could not issue. Writing them together makes that
    /// state unrepresentable rather than merely unlikely.
    ///
    /// `'failed'` is an accepted prior status, not just `'created'`: a renter whose first
    /// attempt was declined retries on the *same* session, and refusing that transition
    /// would leave a paid booking stuck as failed.
    async fn payment_succeeded(
        &self,
        payment_id: Uuid,
        intent_id: String,
        at: DateTime<Utc>,
        seq: u64,
    ) -> MyResult<()> {
        self.db
            .query(
                "BEGIN;
                 UPDATE type::record('payment', $id)
                     SET status = 'succeeded', intent_id = $intent_id
                     WHERE status IN ['created', 'failed'];
                 UPSERT _projection:PAYMENTS SET last_seq = $seq, updated_at = $at;
                 COMMIT;",
            )
            .bind(("id", payment_id))
            .bind(("intent_id", intent_id))
            .bind(("seq", seq as i64))
            .bind(("at", Datetime::from(at)))
            .await?
            .check()?;
        Ok(())
    }

    /// Money states only ever move forwards, which is what makes a redelivered event a
    /// no-op instead of, say, un-refunding a payment.
    async fn payment_status(
        &self,
        payment_id: Uuid,
        to: &str,
        refund_id: Option<String>,
        at: DateTime<Utc>,
        seq: u64,
    ) -> MyResult<()> {
        // 'refunded' comes from 'succeeded'. 'expired' comes from an unpaid session, which
        // is 'created' *or* 'failed' — a booking whose renter was declined and then walked
        // away still has a live session that has to be voided.
        let from: &[&str] = if to == "refunded" {
            &["succeeded"]
        } else {
            &["created", "failed"]
        };
        self.db
            .query(
                "BEGIN;
                 UPDATE type::record('payment', $id)
                     SET status = $to, refund_id = $refund_id ?? refund_id
                     WHERE status IN $from;
                 UPSERT _projection:PAYMENTS SET last_seq = $seq, updated_at = $at;
                 COMMIT;",
            )
            .bind(("id", payment_id))
            .bind(("to", to.to_string()))
            .bind((
                "from",
                from.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            ))
            .bind(("refund_id", refund_id))
            .bind(("seq", seq as i64))
            .bind(("at", Datetime::from(at)))
            .await?
            .check()?;
        Ok(())
    }

    async fn payout(
        &self,
        payout_id: Uuid,
        owner_id: Uuid,
        amount_cents: i64,
        requested_at: DateTime<Utc>,
        at: DateTime<Utc>,
        seq: u64,
    ) -> MyResult<()> {
        self.db
            .query(
                "BEGIN;
                 UPSERT type::record('payout', $id) CONTENT {
                     owner_id: $owner_id, amount_cents: $amount, created_at: $created_at
                 };
                 UPSERT _projection:PAYMENTS SET last_seq = $seq, updated_at = $at;
                 COMMIT;",
            )
            .bind(("id", payout_id))
            .bind(("owner_id", owner_id))
            .bind(("amount", amount_cents))
            .bind(("created_at", Datetime::from(requested_at)))
            .bind(("seq", seq as i64))
            .bind(("at", Datetime::from(at)))
            .await?
            .check()?;
        Ok(())
    }
}
