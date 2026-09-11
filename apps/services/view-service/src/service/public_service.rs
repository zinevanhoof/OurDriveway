use chrono::Utc;
use shared::{
    error::myerror::{ContextExt, MyResult},
    responses::view::{NearbyResponse, PublicSpotResponse},
};
use uuid::Uuid;

use crate::repository::{
    booking_repository::ViewBookingRepository, spot_repository::ViewSpotRepository,
};

/// No relationship required — what any signed-in caller may read about a listing.
///
/// "Public" here means *no predicate on the caller*, not unauthenticated: both routes above
/// this sit behind `AuthedJwt` like every other. The caller's id appears exactly once, in
/// [`Self::nearby`], and it is an **exclusion** rather than a scope — a host is not offered
/// their own driveway as somewhere to park.
pub struct PublicService {
    /// The pool. See [`crate::service::account_service::AccountService::db`].
    pub db: shared::db::Db,
}

impl PublicService {
    /// One active spot as a prospective renter sees it.
    ///
    /// The bookings that come with it are the availability answer and nothing else: which
    /// slots are taken and until when, with no renter, no amount and no hold expiry. A host
    /// looking at their own listing wants [`crate::service::host_service::HostService::spot`]
    /// instead.
    ///
    /// **Two statements**, the second `belonging_to` the first — a join would repeat the
    /// spot's `images`, `address` and `availability` once per booking. See the note in
    /// `repository/mod.rs`.
    ///
    /// The caller is authenticated but not otherwise used: an inactive spot 404s for
    /// everyone here, including its host, because "public spot" is the whole question this
    /// answers. 404 also covers "not visible to you" — a 403 would confirm the existence of
    /// a listing the caller is not allowed to see.
    pub async fn spot(&self, spot_id: Uuid) -> MyResult<PublicSpotResponse> {
        let mut conn = shared::db::conn(&self.db).await?;

        let spot = ViewSpotRepository::find_for_public(&mut conn, spot_id)
            .await?
            .context_not_found(("Not Found", "That spot doesn't exist."))?;

        // `Utc::now()` rather than a client-supplied `$now`. There is no reason to take the
        // instant from the caller when the server has one.
        let bookings =
            ViewBookingRepository::find_for_public_spot(&mut conn, &spot, Utc::now()).await?;

        Ok(PublicSpotResponse::new(spot, bookings))
    }

    /// The map: spots within `meters` of a point.
    ///
    /// **Always excludes the caller's own spots.** `SPOTS_NEARBY` asked for that explicitly
    /// and `SPOTS_IN_RADIUS` did not, which meant the map offered a host their own driveway
    /// as somewhere to park. It is unconditional in the query rather than a flag here.
    ///
    /// The radius is bounded rather than trusted, and that check is here rather than in the
    /// route because it is a rule about what this service will read, not about how a query
    /// string is spelled: an unbounded radius turns the bbox into the whole world and the
    /// index scan into a table scan.
    pub async fn nearby(
        &self,
        caller: Uuid,
        lng: f64,
        lat: f64,
        meters: f64,
    ) -> MyResult<Vec<NearbyResponse>> {
        (meters > 0.0 && meters <= MAX_RADIUS_M).context_bad_request((
            "Invalid radius",
            "Search radius must be between 0 and 50km.",
        ))?;

        let mut conn = shared::db::conn(&self.db).await?;

        let spots =
            ViewSpotRepository::find_pins_for_public(&mut conn, caller, lng, lat, meters).await?;

        Ok(spots.into_iter().map(NearbyResponse::from).collect())
    }
}

/// Ceiling on a radius search, in metres.
///
/// The map's viewport is bounded, so this is not a limit any real client reaches — it is
/// what stops `?meters=40000000` from asking for the planet.
const MAX_RADIUS_M: f64 = 50_000.0;
