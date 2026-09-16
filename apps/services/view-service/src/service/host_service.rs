use chrono::Utc;
use shared::{
    error::myerror::{ContextExt, MyResult},
    responses::view::{
        BalanceResponse, HostBookingsPageResponse, HostSpotListItemResponse, HostSpotResponse,
    },
};
use uuid::Uuid;

use crate::{
    policy,
    repository::{
        booking_repository::ViewBookingRepository, spot_repository::ViewSpotRepository,
        wallet_repository::WalletRepository,
    },
    service::settled_before,
};

/// `host_id = caller` — everything the caller reads as a host.
///
/// One predicate for the whole namespace, and it is on the *spot*: a host's listings, one
/// of their listings, and the money those listings earned. A booking is never reached by
/// id here — it is a child of a spot whose ownership the parent statement already proved,
/// which is why [`Self::spot`]'s second read carries no caller at all.
pub struct HostService {
    /// The pool. See [`crate::service::account_service::AccountService::db`].
    pub db: shared::db::Db,
}

impl HostService {
    /// The caller's own listings, newest first.
    ///
    /// Includes their inactive spots, which is what the live switch is for, and excludes
    /// their deleted ones.
    pub async fn spots(&self, host_id: Uuid) -> MyResult<Vec<HostSpotListItemResponse>> {
        let mut conn = shared::db::conn(&self.db).await?;

        let spots = ViewSpotRepository::find_list_for_host(&mut conn, host_id).await?;

        Ok(spots
            .into_iter()
            .map(HostSpotListItemResponse::from)
            .collect())
    }

    /// One spot as its host sees it, with the slots still taken on it.
    ///
    /// **Two statements**, and the second is keyed off the first. The booking *rows* are
    /// not here — who is coming is [`Self::spot_bookings`], a separate read with its own
    /// cache lifetime. What stays is one merged `booked` map, which the edit form needs to
    /// warn a host before they remove hours someone has taken.
    ///
    /// A non-host gets 404, not 403 — a 403 would confirm the existence of a listing the
    /// caller is not allowed to see.
    pub async fn spot(&self, spot_id: Uuid, host_id: Uuid) -> MyResult<HostSpotResponse> {
        let mut conn = shared::db::conn(&self.db).await?;

        let spot = ViewSpotRepository::find_for_host(&mut conn, spot_id, host_id)
            .await?
            .context_not_found(("Not Found", "That spot doesn't exist."))?;

        // `Utc::now()` rather than a client-supplied `$now`. Three of the four GraphQL
        // documents this replaces made the browser pass one, which meant a client could ask
        // what was booked at any time it liked — harmless, but there is no reason to take the
        // instant from the caller when the server has one.
        let booked =
            ViewBookingRepository::find_booked_for_host_spot(&mut conn, &spot, Utc::now()).await?;

        Ok(HostSpotResponse::new(spot, booked))
    }

    /// One window of one spot's bookings: the manage screen's two-row preview and the
    /// paged screen behind it.
    ///
    /// **Three statements, and the first one is the authorization.** It is the same
    /// `find_for_host` as [`Self::spot`] — a booking is reachable here only as a child
    /// of a spot whose ownership that statement proved, so neither the count nor the
    /// window carries a caller. A non-host gets 404 from the first statement and never
    /// reaches the other two.
    ///
    /// Every parameter is parsed by [`policy::bookings`], which is also what decides that
    /// an unknown tab or status, a limit out of range or a negative offset is a 422
    /// rather than something silently clamped.
    pub async fn spot_bookings(
        &self,
        spot_id: Uuid,
        host_id: Uuid,
        scope: Option<String>,
        status: Option<String>,
        limit: Option<i64>,
        offset: Option<i64>,
    ) -> MyResult<HostBookingsPageResponse> {
        let parsed = (|| {
            Some((
                policy::bookings::scope(scope.as_deref())?,
                policy::bookings::statuses(status.as_deref())?,
                policy::bookings::limit(limit)?,
                policy::bookings::offset(offset)?,
            ))
        })();
        let (past, statuses, limit, offset) = parsed.context_bad_request((
            "Invalid page",
            "Ask for scope=upcoming or scope=past, status from reserved, confirmed, cancelled, released, a limit from 1 to 50, and an offset from 0.",
        ))?;

        let mut conn = shared::db::conn(&self.db).await?;

        let spot = ViewSpotRepository::find_for_host(&mut conn, spot_id, host_id)
            .await?
            .context_not_found(("Not Found", "That spot doesn't exist."))?;

        // One instant for both statements. Read twice, a booking ending in this second
        // could be counted in one tab and listed in the other.
        let now = Utc::now();

        let total =
            ViewBookingRepository::count_for_host_spot(&mut conn, &spot, now, past, &statuses)
                .await?;
        let bookings = ViewBookingRepository::find_page_for_host_spot(
            &mut conn, &spot, now, past, &statuses, limit, offset,
        )
        .await?;

        Ok(HostBookingsPageResponse {
            bookings: bookings.into_iter().map(Into::into).collect(),
            next_offset: policy::bookings::next_offset(offset, limit, total),
            total,
        })
    }

    /// What the caller has earned, withdrawn and is waiting on.
    ///
    /// This was `GET /api/payment/earnings`. It moved with every other read:
    /// payment-service writes, view-service reads. What did **not** move is the figure a
    /// withdrawal actually pays out — `PaymentService::request_payout` still computes that
    /// inside its own transaction, under a lock, from its own tables, so nothing is ever
    /// spent against a projection that may be a moment behind.
    ///
    /// A renter who has never hosted gets zeroes, which is the honest answer rather than a
    /// 404.
    pub async fn balance(&self, host_id: Uuid) -> MyResult<BalanceResponse> {
        let mut conn = shared::db::conn(&self.db).await?;

        let balance =
            WalletRepository::balance_for_host(&mut conn, host_id, settled_before(Utc::now()))
                .await?;

        Ok(BalanceResponse::from(balance))
    }
}
