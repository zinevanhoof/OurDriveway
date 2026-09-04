use shared::domain_models::view::payment::{ViewPayment, ViewPaymentPatch};
use shared::error::myerror::MyResult;
use sqlx::PgExecutor;
use uuid::Uuid;

/// The `payment` table in the read model.
///
/// Write statements only. Nothing reads this table by id — the one read of it is the
/// wallet, and that spans four sources at once, so it lives in
/// [`crate::repository::wallet_repository`] rather than being assembled from per-table
/// lookups here.
pub struct ViewPaymentRepository;

impl ViewPaymentRepository {
    /// Insert-or-replace the whole row, from `PaymentCreated`.
    ///
    /// Idempotent by construction, which is what lets the projector replay the event.
    pub async fn upsert(ex: impl PgExecutor<'_>, payment: ViewPayment) -> MyResult<()> {
        sqlx::query(
            "INSERT INTO payment
                 (id, version, booking_id, owner_id, renter_id, amount, status,
                  created_at, refunded_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
             ON CONFLICT (id) DO UPDATE SET
                 version     = EXCLUDED.version,
                 booking_id  = EXCLUDED.booking_id,
                 owner_id    = EXCLUDED.owner_id,
                 renter_id   = EXCLUDED.renter_id,
                 amount      = EXCLUDED.amount,
                 status      = EXCLUDED.status,
                 created_at  = EXCLUDED.created_at,
                 refunded_at = EXCLUDED.refunded_at",
        )
        .bind(payment.id)
        .bind(payment.version as i64)
        .bind(payment.booking_id)
        .bind(payment.owner_id)
        .bind(payment.renter_id)
        .bind(payment.amount)
        .bind(payment.status)
        .bind(payment.created_at)
        .bind(payment.refunded_at)
        .execute(ex)
        .await?;
        Ok(())
    }

    /// Patch a payment's status, and the refund instant that comes with one of them.
    ///
    /// **No `from` guard**, unlike `ViewBookingRepository::settle`. This is a
    /// projection of a log that is already ordered per payment — one booking's payment
    /// events share a subject and therefore a projector lane — so there is no
    /// out-of-order transition for a guard to refuse. `db::set_version` is what makes a
    /// redelivery a no-op.
    ///
    /// A patch for a payment this projection has not seen updates nothing and is not an
    /// error: the row arrives with `Created`, and if PAYMENTS expired before that
    /// reached us, `POST /internal/backfill` on payment-service re-emits it.
    ///
    /// `COALESCE($n, column)` is absent-is-unchanged, and is also the ceiling: no patch
    /// can set a column back to NULL. Nothing un-refunds a payment.
    pub async fn transition(
        ex: impl PgExecutor<'_>,
        payment_id: Uuid,
        patch: ViewPaymentPatch,
    ) -> MyResult<()> {
        sqlx::query(
            "UPDATE payment SET
                 status      = COALESCE($2, status),
                 refunded_at = COALESCE($3, refunded_at)
             WHERE id = $1",
        )
        .bind(payment_id)
        .bind(patch.status)
        .bind(patch.refunded_at)
        .execute(ex)
        .await?;
        Ok(())
    }
}
