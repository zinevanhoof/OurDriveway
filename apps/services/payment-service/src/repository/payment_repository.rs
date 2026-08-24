use std::sync::Arc;

use chrono::DateTime;
use chrono::Utc;
use shared::db::Querier;
use shared::domain_models::payment::{Payment, PaymentPatch};
use shared::error::myerror::MyResult;
use surrealdb::{Surreal, engine::remote::ws::Client, types::Datetime};
use uuid::Uuid;

/// The `payment` table.
///
/// Every method is a statement written out in full. There are five of them because
/// five is what this service calls — a lookup by each of the two UNIQUE columns, the
/// write, the conditional transition, and the earnings sum.
///
/// Generic over its querier so the same type serves both positions: the service
/// holds one over the pooled `Surreal<Client>`, a projector builds one over the open
/// `&Transaction` for a single event.
pub struct PaymentRepository<Q: Querier = Arc<Surreal<Client>>> {
    pub q: Q,
}

impl<Q: Querier> PaymentRepository<Q> {
    /// `payment_booking … UNIQUE`, so `LIMIT 1` here is a fact about the schema and
    /// not a hope about the data.
    ///
    /// `*` takes every column, so a new field on [`Payment`] needs no edit here.
    /// Only `id` is spelled out, because SurrealDB returns it as the record key
    /// `payment:⟨uuid⟩` while the struct holds a plain uuid — and an explicit alias
    /// beats `*` for the same name in either order, checked against 3.2.4 rather
    /// than assumed.
    pub async fn find_by_booking_id(&self, booking_id: Uuid) -> MyResult<Option<Payment>> {
        Ok(self
            .q
            .q("SELECT record::id(id) AS id, * FROM ONLY payment
                WHERE booking_id = $v LIMIT 1")
            .bind(("v", booking_id))
            .await?
            .take(0)?)
    }

    /// `payment_session … UNIQUE`. The checkout screen knows only a session id.
    pub async fn find_by_session_id(&self, session_id: String) -> MyResult<Option<Payment>> {
        Ok(self
            .q
            .q("SELECT record::id(id) AS id, * FROM ONLY payment
                WHERE session_id = $v LIMIT 1")
            .bind(("v", session_id))
            .await?
            .take(0)?)
    }

    /// Insert-or-replace the whole row, keyed by its own id.
    ///
    /// Idempotent by construction, which is what lets a projector replay the same
    /// event. `CONTENT $row` binds the struct whole rather than column by column, so
    /// adding a field to [`Payment`] needs no change here.
    ///
    /// The row carries its own `id` and the statement also names one. SurrealDB
    /// requires them to agree and errors if they do not — verified against 3.2.4,
    /// which makes this a free assertion rather than a risk.
    pub async fn upsert(&self, payment: Payment) -> MyResult<()> {
        let id = payment.id;
        self.q
            .q("UPSERT type::record('payment', $id) CONTENT $row")
            .bind(("id", id))
            .bind(("row", payment))
            .await?
            .check()?;
        Ok(())
    }

    /// Patch a payment only if it is currently in one of `from`.
    ///
    /// The whole value is the `WHERE`. Money states only ever move forwards, which
    /// is what makes a redelivered event a no-op instead of, say, un-refunding a
    /// payment.
    ///
    /// `?? column` is what makes an absent field mean "unchanged" rather than
    /// "clear it", and is also the ceiling: no patch can set a column back to NONE.
    /// The four columns here are every column [`PaymentPatch`] carries — add one
    /// there and it has to be added here too.
    pub async fn transition(
        &self,
        payment_id: Uuid,
        from: &[&str],
        patch: PaymentPatch,
    ) -> MyResult<()> {
        patch
            .bind(
                self.q
                    .q("UPDATE type::record('payment', $v) SET
                            status         = $status         ?? status,
                            intent_id      = $intent_id      ?? intent_id,
                            refund_id      = $refund_id      ?? refund_id,
                            failure_reason = $failure_reason ?? failure_reason
                        WHERE status IN $from;")
                    .bind(("v", payment_id))
                    .bind((
                        "from",
                        from.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
                    )),
            )
            .await?
            .check()?;
        Ok(())
    }

    /// A host's settled income: paid, not refunded, and for a booking that has
    /// actually happened.
    ///
    /// Two conditions and both are needed. The payment must have succeeded; the
    /// *booking* must still be confirmed and old enough to have settled. The booking
    /// half is what stops a host withdrawing money for a booking that has not
    /// happened yet — see SETTLEMENT_SECS in `Config`.
    ///
    /// `cutoff` is passed in rather than read from a clock here, so this stays a pure
    /// query and the caller owns the window.
    pub async fn earned(&self, owner_id: &Uuid, cutoff: DateTime<Utc>) -> MyResult<i64> {
        let sum: Option<i64> = self
            .q
            .q("SELECT VALUE math::sum(amount_cents) FROM ONLY (
                    SELECT amount_cents FROM payment
                    WHERE owner_id = $o AND status = 'succeeded'
                      AND booking_id IN (
                          SELECT VALUE record::id(id) FROM booking
                          WHERE owner_id = $o AND status = 'confirmed' AND ends_at < $cutoff
                      )
                ) GROUP ALL")
            .bind(("o", *owner_id))
            .bind(("cutoff", Datetime::from(cutoff)))
            .await?
            .take(0)?;
        Ok(sum.unwrap_or(0))
    }
}
