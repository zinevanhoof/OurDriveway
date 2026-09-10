use chrono::{DateTime, Utc};
use diesel::prelude::*;
use uuid::Uuid;

use crate::{domain_models::payment::status, events::payment::PaymentCreated};

/// The `payment` table in the read model — what was charged, and what came back.
///
/// This exists because the wallet has to show a renter what they spent and a host what
/// they earned, and both are payment-service's rows. It carries **no Stripe handles**:
/// the session, intent and refund ids are how the write side talks to Stripe, and
/// nothing a browser reaches has any use for them.
///
/// The rule this reverses is recorded in `migrations/view/0003_payment/up.sql`. In short:
/// this projection is displayed, never spent — `request_payout` computes what it pays
/// out from payment-service's own tables, inside its own transaction.
#[derive(Clone, Debug, Queryable, Selectable, Insertable, AsChangeset)]
#[diesel(table_name = crate::schema::view::payment)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct ViewPayment {
    pub id: Uuid,
    /// payment-service's version of this payment, as last applied here. What
    /// `bus::await_version` compares a client's `X-Await-Version` against.
    pub version: i64,
    pub booking_id: Uuid,
    /// Who earns it.
    pub host_id: Uuid,
    /// Who paid.
    pub renter_id: Uuid,
    /// EUR cents.
    pub amount: i64,
    pub status: String,
    /// When the checkout session was made, which is where a wallet dates the charge.
    pub created_at: DateTime<Utc>,
    /// When the money went back. `None` unless `status` is `refunded`.
    pub refunded_at: Option<DateTime<Utc>>,
}

impl ViewPayment {
    /// The row a `Created` writes.
    ///
    /// `created_at` comes off the event rather than from `at`, unlike `ViewBooking`:
    /// payment-service stores that exact instant on its own row, and a wallet showing a
    /// different date from the one the write side recorded is a bug nobody would spot.
    pub fn created(e: PaymentCreated, version: i64) -> Self {
        Self {
            id: e.payment_id,
            version,
            booking_id: e.booking_id,
            host_id: e.host_id,
            renter_id: e.renter_id,
            amount: e.amount_cents,
            status: status::CREATED.to_string(),
            created_at: e.created_at,
            refunded_at: None,
        }
    }
}

/// A partial update to a [`ViewPayment`], written with the struct-update idiom.
///
/// Two columns, which is every column that changes after creation. The Stripe handles
/// that make up the rest of payment-service's own patch have no counterpart here
/// because there are no columns to put them in.
#[derive(Debug, Default, AsChangeset)]
#[diesel(table_name = crate::schema::view::payment)]
pub struct ViewPaymentPatch {
    pub status: Option<String>,
    pub refunded_at: Option<DateTime<Utc>>,
}

impl ViewPaymentPatch {
    // No `bind` — `AsChangeset` is the `SET` list. See the note in
    // `domain_models::user::user`.

    /// Paid. The intent id that arrives with this event stops at the write side.
    pub fn succeeded() -> Self {
        Self {
            status: Some(status::SUCCEEDED.to_string()),
            ..Self::default()
        }
    }

    /// An attempt was declined. Not terminal — the renter may pay the same session
    /// with another card — and `failure_reason` is deliberately not projected: it is
    /// Stripe's message for our log, never for a screen.
    pub fn failed() -> Self {
        Self {
            status: Some(status::FAILED.to_string()),
            ..Self::default()
        }
    }

    /// Money returned, and when. Paired for the same reason payment-service's own
    /// `refunded` patch pairs them: a refunded row that cannot say when is one the
    /// wallet has no month to file it under.
    pub fn refunded(refunded_at: DateTime<Utc>) -> Self {
        Self {
            status: Some(status::REFUNDED.to_string()),
            refunded_at: Some(refunded_at),
            ..Self::default()
        }
    }

    /// The session was voided before anyone paid.
    pub fn expired() -> Self {
        Self {
            status: Some(status::EXPIRED.to_string()),
            ..Self::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The literal is **exhaustive on purpose** — no `..Default::default()`. Add a
    /// field to [`ViewPaymentPatch`] and this stops compiling, which is the reminder
    /// that the `SET` list in `ViewPaymentRepository::transition` needs it too, and a
    /// `.bind()` in the matching position.
    #[test]
    fn set_covers_every_patchable_column() {
        let _: ViewPaymentPatch = ViewPaymentPatch {
            status: None,
            refunded_at: None,
        };
    }

    /// Only a refund carries an instant. Anything else setting `refunded_at` would put
    /// a second row in a wallet for a payment that was never given back.
    #[test]
    fn only_refunding_sets_refunded_at() {
        assert!(ViewPaymentPatch::refunded(Utc::now()).refunded_at.is_some());
        for patch in [
            ViewPaymentPatch::succeeded(),
            ViewPaymentPatch::failed(),
            ViewPaymentPatch::expired(),
        ] {
            assert!(patch.refunded_at.is_none());
        }
    }
}
