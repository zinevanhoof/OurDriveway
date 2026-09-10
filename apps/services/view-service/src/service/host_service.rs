use chrono::Utc;
use shared::{
    error::myerror::{ContextExt, MyResult},
    responses::view::{BalanceResponse, HostSpotListItemResponse, HostSpotResponse},
};
use uuid::Uuid;

use crate::{
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

    /// One spot as its host sees it, with the bookings on it.
    ///
    /// **Two statements**, and the second is `belonging_to` the first. A join would repeat
    /// this spot's `images`, `address` and `availability` once per booking, and those are
    /// the expensive columns. They also get independent cache keys this way, so a booking
    /// landing invalidates the availability without refetching the listing.
    ///
    /// The second statement carries no caller and no status filter: `host_id = $2` in the
    /// first one already settled who is asking, and a host is entitled to their own
    /// cancelled rows.
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
        let bookings =
            ViewBookingRepository::find_for_host_spot(&mut conn, &spot, Utc::now()).await?;

        Ok(HostSpotResponse::new(spot, bookings))
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
