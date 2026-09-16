use chrono::Utc;
use shared::{
    error::myerror::{ContextExt, MyResult},
    responses::view::{
        NextBookingResponse, RenterBookingResponse, RenterBookingsPageResponse,
        RenterSpotResponse,
    },
};
use uuid::Uuid;

use crate::{
    policy,
    repository::{booking_repository::ViewBookingRepository, spot_repository::ViewSpotRepository},
    service::BAD_BOOKINGS_PAGE,
};

/// `renter_id = caller` — everything the caller reads as a renter.
///
/// Their bookings, the next one due, one by id, and a spot they booked. Every statement
/// carries the caller itself; unlike `host`, there is no parent read whose success proves
/// anything for the next one.
pub struct RenterService {
    /// The pool. See [`crate::service::account_service::AccountService::db`].
    pub db: shared::db::Db,
}

impl RenterService {
    /// One spot the caller has booked. 404 for any spot they never booked, so a renter
    /// cannot use this to read a paused listing they have no business with.
    pub async fn spot(&self, spot_id: Uuid, renter_id: Uuid) -> MyResult<RenterSpotResponse> {
        let mut conn = shared::db::conn(&self.db).await?;

        let spot = ViewSpotRepository::find_for_renter(&mut conn, spot_id, renter_id)
            .await?
            .context_not_found(("Not Found", "That spot doesn't exist."))?;

        Ok(RenterSpotResponse::from(spot))
    }

    /// One window of the caller's own bookings, each with its spot card — the one
    /// exception to spots and bookings being separate reads.
    pub async fn bookings(
        &self,
        renter_id: Uuid,
        scope: Option<String>,
        status: Option<String>,
        limit: Option<i64>,
        offset: Option<i64>,
    ) -> MyResult<RenterBookingsPageResponse> {
        let (past, statuses, limit, offset) =
            policy::bookings::window(scope.as_deref(), status.as_deref(), limit, offset)
                .context_bad_request(BAD_BOOKINGS_PAGE)?;

        let mut conn = shared::db::conn(&self.db).await?;

        // One instant for both statements, as on the host's list.
        let now = Utc::now();

        let total =
            ViewBookingRepository::count_for_renter(&mut conn, renter_id, now, past, &statuses)
                .await?;
        let bookings = ViewBookingRepository::find_page_for_renter(
            &mut conn, renter_id, now, past, &statuses, limit, offset,
        )
        .await?;

        Ok(RenterBookingsPageResponse {
            bookings: bookings.into_iter().map(Into::into).collect(),
            next_offset: policy::page::next_offset(offset, limit, total),
            total,
        })
    }

    /// The caller's soonest confirmed booking that has not ended, or `None`.
    pub async fn next(&self, renter_id: Uuid) -> MyResult<Option<NextBookingResponse>> {
        let mut conn = shared::db::conn(&self.db).await?;

        Ok(
            ViewBookingRepository::find_next_for_renter(&mut conn, renter_id, Utc::now())
                .await?
                .map(NextBookingResponse::from),
        )
    }

    /// One of the caller's own bookings, by id.
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
