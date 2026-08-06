use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Json, response::IntoResponse};
use chrono::{DateTime, Utc};
use serde::Serialize;
use shared::error::myerror::MyResult;
use shared::events::{STREAM_BOOKINGS, user::record_key};
use shared::extract::Valid;
use shared::extractors::authed_jwt::AuthedJwt;
use shared::requests::booking::CreateBookingRequest;

use crate::AppState;

#[derive(Serialize)]
pub struct ReservedResponse {
    pub id: String,
    /// `"BOOKINGS:812"` — where this write landed in the log. The client echoes it
    /// back on its next read so a load balancer can't route it to an instance that
    /// hasn't projected this event yet.
    pub seq: String,
    /// When the hold lapses. Returned here so the checkout countdown needs no
    /// follow-up query.
    pub expires_at: DateTime<Utc>,
    /// EUR cents, recomputed server-side from the authorised minutes.
    pub amount_cents: i64,
}

#[derive(Serialize)]
pub struct AcceptedResponse {
    pub seq: String,
}

/// 202, not 201: the event is committed to the log, but the projections that
/// answer reads are still catching up. `seq` is how a caller waits for its own
/// write.
pub async fn reserve(
    AuthedJwt { user_id, .. }: AuthedJwt,
    State(state): State<AppState>,
    Valid(request): Valid<CreateBookingRequest>,
) -> MyResult<impl IntoResponse> {
    // Renter identity comes from the verified token, never from the request body.
    let reserved = state.booking_service.reserve(request, user_id).await?;

    Ok((
        StatusCode::ACCEPTED,
        Json(ReservedResponse {
            id: record_key(&reserved.booking_id),
            seq: format!("{STREAM_BOOKINGS}:{}", reserved.seq),
            expires_at: reserved.expires_at,
            amount_cents: reserved.amount_cents,
        }),
    ))
}

/// Payment succeeded. Stands in for the provider's callback until one exists —
/// swapping in a webhook later changes only how the caller is authenticated.
pub async fn confirm(
    AuthedJwt { user_id, .. }: AuthedJwt,
    State(state): State<AppState>,
    Path(booking_id): Path<String>,
) -> MyResult<impl IntoResponse> {
    let seq = state.booking_service.confirm(&booking_id, &user_id).await?;
    Ok(accepted(seq))
}

/// The renter backed out of checkout. Frees the slots now rather than making the
/// next renter wait out the hold.
pub async fn release(
    AuthedJwt { user_id, .. }: AuthedJwt,
    State(state): State<AppState>,
    Path(booking_id): Path<String>,
) -> MyResult<impl IntoResponse> {
    let seq = state.booking_service.release(&booking_id, &user_id).await?;
    Ok(accepted(seq))
}

/// The renter withdraws a booking they already paid for, up to an hour before it
/// starts.
///
/// Its own endpoint rather than letting DELETE dispatch on status: a client that
/// means "abandon my hold" must never cancel a paid booking because the payment
/// landed between rendering the button and pressing it.
pub async fn cancel(
    AuthedJwt { user_id, .. }: AuthedJwt,
    State(state): State<AppState>,
    Path(booking_id): Path<String>,
) -> MyResult<impl IntoResponse> {
    let seq = state.booking_service.cancel(&booking_id, &user_id).await?;
    Ok(accepted(seq))
}

fn accepted(seq: u64) -> impl IntoResponse {
    (
        StatusCode::ACCEPTED,
        Json(AcceptedResponse {
            seq: format!("{STREAM_BOOKINGS}:{seq}"),
        }),
    )
}
