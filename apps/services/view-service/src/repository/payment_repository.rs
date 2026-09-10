use diesel::prelude::*;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use shared::domain_models::view::payment::{ViewPayment, ViewPaymentPatch};
use shared::error::myerror::MyResult;
use shared::schema::view::payment;
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
    pub async fn upsert(conn: &mut AsyncPgConnection, row: ViewPayment) -> MyResult<()> {
        diesel::insert_into(payment::table)
            .values(row.clone())
            .on_conflict(payment::id)
            .do_update()
            .set(row)
            .execute(conn)
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
        conn: &mut AsyncPgConnection,
        payment_id: Uuid,
        patch: ViewPaymentPatch,
    ) -> MyResult<()> {
        diesel::update(payment::table.find(payment_id))
            .set(&patch)
            .execute(conn)
            .await?;
        Ok(())
    }
}
