use chrono::Utc;
use shared::{
    error::myerror::{ContextExt, MyResult},
    general_models::booking::Booked,
    responses::view::{
        BalanceResponse, HostBookingsPageResponse, HostSpotResponse, HostSpotSummaryResponse,
        HostSpotsPageResponse, HostSummaryResponse,
    },
};
use uuid::Uuid;

use crate::{
    policy,
    repository::{
        booking_repository::ViewBookingRepository, spot_repository::ViewSpotRepository,
        wallet_repository::WalletRepository,
    },
    service::{BAD_BOOKINGS_PAGE, BAD_PAGE, settled_before},
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
    /// One window of the caller's own listings, newest first.
    ///
    /// Includes their inactive spots, which is what the live switch is for, and excludes
    /// their deleted ones.
    pub async fn spots(
        &self,
        host_id: Uuid,
        limit: Option<i64>,
        offset: Option<i64>,
    ) -> MyResult<HostSpotsPageResponse> {
        let (limit, offset) = policy::page::window(limit, offset).context_bad_request(BAD_PAGE)?;

        let mut conn = shared::db::conn(&self.db).await?;

        let total = ViewSpotRepository::count_for_host(&mut conn, host_id).await?;
        let spots = ViewSpotRepository::find_page_for_host(&mut conn, host_id, limit, offset).await?;

        Ok(HostSpotsPageResponse {
            spots: spots.into_iter().map(Into::into).collect(),
            next_offset: policy::page::next_offset(offset, limit, total),
            total,
        })
    }

    /// The caller's totals as a host: listings and how many are live and occupied,
    /// completed bookings, and earned — ever, this month and last month.
    ///
    /// Five statements on one connection and one `now`, so the figures describe the same
    /// moment. A caller who has never hosted gets zeroes, which is the honest answer.
    pub async fn summary(&self, host_id: Uuid) -> MyResult<HostSummaryResponse> {
        let mut conn = shared::db::conn(&self.db).await?;
        let now = Utc::now();

        // This month and the one before it, as the wallet draws them: UTC months.
        let (this_start, this_end) = policy::wallet::bounds(&policy::wallet::label(now))
            .expect("the label of a real instant is a real month");
        let (last_start, _) =
            policy::wallet::bounds(&policy::wallet::label(this_start - chrono::Duration::seconds(1)))
                .expect("the label of a real instant is a real month");

        let (spots, active_spots) = ViewSpotRepository::counts_for_host(&mut conn, host_id).await?;
        let stats = ViewBookingRepository::stats_for_host(&mut conn, host_id, now).await?;
        let earned_cents = WalletRepository::earned_for_host(&mut conn, host_id).await?;
        let (earned_this_month_cents, earned_last_month_cents) =
            WalletRepository::earned_by_month_for_host(
                &mut conn, host_id, last_start, this_start, this_end,
            )
            .await?;

        // Listings, not bookings: two bookings in one driveway at once is still one
        // driveway occupied.
        let unfinished = ViewBookingRepository::find_unfinished_for_host(&mut conn, host_id, now).await?;
        let booked_now = unfinished
            .iter()
            .filter(|(_, booked, tz)| policy::occupancy::active_now(booked, tz, now))
            .map(|(spot_id, _, _)| spot_id)
            .collect::<std::collections::HashSet<_>>()
            .len() as i64;

        Ok(HostSummaryResponse {
            spots,
            active_spots,
            booked_now,
            bookings: stats.bookings,
            earned_cents,
            earned_this_month_cents,
            earned_last_month_cents,
        })
    }

    /// How one of the caller's spots is doing: completed bookings, earned, rating.
    ///
    /// **The first statement is the authorization** — the same `find_for_host` as
    /// [`Self::spot`], so a non-host gets the same 404 and never reaches the aggregates.
    pub async fn spot_summary(
        &self,
        spot_id: Uuid,
        host_id: Uuid,
    ) -> MyResult<HostSpotSummaryResponse> {
        let mut conn = shared::db::conn(&self.db).await?;

        let spot = ViewSpotRepository::find_for_host(&mut conn, spot_id, host_id)
            .await?
            .context_not_found(("Not Found", "That spot doesn't exist."))?;

        let stats = ViewBookingRepository::stats_for_host_spot(&mut conn, &spot, Utc::now()).await?;
        let earned_cents =
            WalletRepository::earned_for_host_spot(&mut conn, host_id, spot.id).await?;

        Ok(HostSpotSummaryResponse {
            bookings: stats.bookings,
            earned_cents,
            rating: policy::rating::average(stats.rating_sum, stats.ratings),
            ratings: stats.ratings,
        })
    }

    /// One spot as its host sees it. The spot only — its bookings are
    /// [`Self::spot_bookings`] and its taken slots [`Self::booked`].
    ///
    /// A non-host gets 404, not 403 — a 403 would confirm the existence of a listing the
    /// caller is not allowed to see.
    pub async fn spot(&self, spot_id: Uuid, host_id: Uuid) -> MyResult<HostSpotResponse> {
        let mut conn = shared::db::conn(&self.db).await?;

        let spot = ViewSpotRepository::find_for_host(&mut conn, spot_id, host_id)
            .await?
            .context_not_found(("Not Found", "That spot doesn't exist."))?;

        Ok(HostSpotResponse::from(spot))
    }

    /// Every slot a reserved or confirmed booking still holds on one spot, merged into
    /// one map. What the edit form checks before a host removes hours someone has taken.
    ///
    /// **Two statements, and the first is the authorization** — the same `find_for_host`
    /// as [`Self::spot`], so a non-host gets the same 404.
    pub async fn booked(&self, spot_id: Uuid, host_id: Uuid) -> MyResult<Booked> {
        let mut conn = shared::db::conn(&self.db).await?;

        let spot = ViewSpotRepository::find_for_host(&mut conn, spot_id, host_id)
            .await?
            .context_not_found(("Not Found", "That spot doesn't exist."))?;

        // `Utc::now()` rather than a client-supplied `$now`. Three of the four GraphQL
        // documents this replaces made the browser pass one, which meant a client could ask
        // what was booked at any time it liked — harmless, but there is no reason to take the
        // instant from the caller when the server has one.
        let rows =
            ViewBookingRepository::find_booked_for_host_spot(&mut conn, &spot, Utc::now()).await?;

        let mut merged = Booked::new();
        for (date, slots) in rows.into_iter().flatten() {
            merged.entry(date).or_default().extend(slots);
        }
        Ok(merged)
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
    /// Every parameter is parsed by [`policy::bookings`] and [`policy::page`], which are
    /// also what decide that an unknown tab or status, a limit out of range or a negative
    /// offset is a 422 rather than something silently clamped.
    pub async fn spot_bookings(
        &self,
        spot_id: Uuid,
        host_id: Uuid,
        scope: Option<String>,
        status: Option<String>,
        limit: Option<i64>,
        offset: Option<i64>,
    ) -> MyResult<HostBookingsPageResponse> {
        let (past, statuses, limit, offset) =
            policy::bookings::window(scope.as_deref(), status.as_deref(), limit, offset)
                .context_bad_request(BAD_BOOKINGS_PAGE)?;

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
            next_offset: policy::page::next_offset(offset, limit, total),
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
