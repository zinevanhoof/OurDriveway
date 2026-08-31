use axum::{
    Json,
    extract::{Path, State},
    response::IntoResponse,
};
use shared::{
    error::myerror::{ContextExt, MyResult},
    extractors::authed_jwt::AuthedJwt,
};
use uuid::Uuid;

use crate::{AppState, repository::booking_repository::ViewBookingRepository};

/// `GET /api/view/bookings/:id` — one booking, whole, for a party to it.
///
/// Replaces `BOOKING_STATUS`, which asked for the status alone because that was all the
/// payment-return screen needed. It is one row either way, so the aggregate comes back
/// and the next screen that wants a field does not need a new document.
///
/// A caller who is neither the renter nor the host gets 404. That is stricter than what
/// this route used to do — it handed a stranger the four public fields of a `reserved`
/// or `confirmed` booking — and nothing needed the looser behaviour: availability is
/// answered by the spot's own read.
pub async fn detail(
    AuthedJwt { user_id, .. }: AuthedJwt,
    State(state): State<AppState>,
    Path(booking_id): Path<Uuid>,
) -> MyResult<impl IntoResponse> {
    // 404 for both "no such booking" and "not yours", same as spots.
    let booking = ViewBookingRepository::find_owner_by_id(&state.db, booking_id, user_id)
        .await?
        .context_not_found(("Not Found", "That booking doesn't exist."))?;

    Ok(Json(booking))
}
