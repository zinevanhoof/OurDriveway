use chrono::Utc;
use shared::{
    error::myerror::{ContextExt, MyResult},
    responses::view::{NextBookingResponse, RenterBookingResponse},
};
use uuid::Uuid;

use crate::repository::booking_repository::ViewBookingRepository;

/// `renter_id = caller` — everything the caller reads as a renter.
///
/// Three reads over one projection and one join, which is why `RenterBookingProjection`
/// carries that join on itself via `HasQuery`. They differ in `WHERE` and `LIMIT`, not in
/// shape.
///
/// They do **not** share a response. `/next` renders a card with four fields and says so;
/// sending it a full booking because the query happened to select one is how a wire
/// contract stops meaning anything.
pub struct RenterService {
    /// The pool. See [`crate::service::account_service::AccountService::db`].
    pub db: shared::db::Db,
}

impl RenterService {
    /// The caller's own bookings, newest first.
    ///
    /// A booking the caller merely *hosts* is deliberately not here — that belongs on the
    /// spot's page under `host`, where the host is already looking at their listing.
    pub async fn bookings(&self, renter_id: Uuid) -> MyResult<Vec<RenterBookingResponse>> {
        let mut conn = shared::db::conn(&self.db).await?;

        let bookings = ViewBookingRepository::find_list_for_renter(&mut conn, renter_id).await?;

        Ok(bookings
            .into_iter()
            .map(RenterBookingResponse::from)
            .collect())
    }

    /// The caller's soonest booking that has not ended, or `None`.
    ///
    /// **This read exists to replace a client-side fold that was wrong.** `HomeView.vue`
    /// fetched the renter's entire booking history to render one card, filtered to
    /// `confirmed`, and ranked what was left by comparing `"YYYY-MM-DDTHH:MM"` wall-clock
    /// strings — which orders wrong across time zones, as that file's own comment said.
    ///
    /// `ends_at` is an instant, so one `ORDER BY … LIMIT 1` is correct in every zone at
    /// once and reads one row.
    ///
    /// `None` when there is nothing coming, rather than a 404: "you have no bookings" is an
    /// answer, and a 404 would make the home screen log an error on a perfectly ordinary
    /// account.
    pub async fn next(&self, renter_id: Uuid) -> MyResult<Option<NextBookingResponse>> {
        let mut conn = shared::db::conn(&self.db).await?;

        Ok(
            ViewBookingRepository::find_next_for_renter(&mut conn, renter_id, Utc::now())
                .await?
                .map(NextBookingResponse::from),
        )
    }

    /// One of the caller's own bookings.
    ///
    /// `renter_id = caller` and nothing else. This used to be
    /// `(renter_id = $2 OR host_id = $2)`, one read answering both parties to a booking;
    /// the host half is [`crate::service::host_service::HostService::spot`] now, which is
    /// where a host is already looking.
    ///
    /// 404 for a booking that is not the caller's, same as everywhere else.
    pub async fn booking(
        &self,
        booking_id: Uuid,
        renter_id: Uuid,
    ) -> MyResult<RenterBookingResponse> {
        let mut conn = shared::db::conn(&self.db).await?;

        let booking = ViewBookingRepository::find_for_renter(&mut conn, booking_id, renter_id)
            .await?
            .context_not_found(("Not Found", "That booking doesn't exist."))?;

        Ok(RenterBookingResponse::from(booking))
    }
}
