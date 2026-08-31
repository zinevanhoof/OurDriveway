use axum::extract::{Path, State};
use axum::response::IntoResponse;
use shared::error::myerror::MyResult;
use shared::extract::Valid;
use shared::extractors::authed_jwt::AuthedJwt;
use shared::requests::booking::CreateBookingRequest;
use shared::responses::common::{accepted, backfilled};
use uuid::Uuid;

use crate::AppState;

/// 202, not 201: the event is committed to the log, but the projections that
/// answer reads are still catching up. `seq` is how a caller waits for its own
/// write.
///
/// The one write in the system that answers with more than a seq, and `id` is now
/// all of it. The hold's `expires_at` and the priced `amount_cents` used to ride
/// along for the checkout drawer's countdown and total; checkout is its own route
/// now and reads both from the Stripe session, so nothing consumed them.
pub async fn create_booking(
    AuthedJwt { user_id, .. }: AuthedJwt,
    State(state): State<AppState>,
    Valid(request): Valid<CreateBookingRequest>,
) -> MyResult<impl IntoResponse> {
    // Renter identity comes from the verified token, never from the request body.
    let created = state
        .booking_service
        .create_booking(&user_id, request)
        .await?;

    Ok(created.accepted())
}

// There is no confirm endpoint. Confirmation is not something a client can ask for:
// it happens when payment-service publishes `PaymentEvent::Succeeded` off a
// signature-verified Stripe webhook, and `worker::PaymentWorker` picks it up. A renter
// able to confirm their own booking would not have to pay for it.

/// The renter backed out of checkout. Frees the slots now rather than making the
/// next renter wait out the hold.
pub async fn release(
    AuthedJwt { user_id, .. }: AuthedJwt,
    State(state): State<AppState>,
    Path(booking_id): Path<Uuid>,
) -> MyResult<impl IntoResponse> {
    let token = state.booking_service.release(&user_id, &booking_id).await?;
    Ok(accepted(token))
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
    Path(booking_id): Path<Uuid>,
) -> MyResult<impl IntoResponse> {
    let token = state.booking_service.cancel(&user_id, &booking_id).await?;
    Ok(accepted(token))
}

/// `POST /internal/backfill` — re-emit every booking, for rebuilding a consumer.
///
/// Off the ingress and unauthenticated by construction; see the same handler in
/// user-service for why that is the whole of the access control.
///
/// The one of the four that wakes a side-effect consumer — payment-service settles
/// on the terminal booking events. `BookingService::backfill` says why that is safe
/// and what would make it stop being.
pub async fn backfill(State(state): State<AppState>) -> MyResult<impl IntoResponse> {
    Ok(backfilled(state.booking_service.backfill().await?))
}
