use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use shared::{
    error::myerror::MyResult,
    extract::Valid,
    extractors::authed_jwt::AuthedJwt,
    requests::payment::CreateSessionRequest,
    responses::common::backfilled,
    responses::payment::{EarningsResponse, PayoutResponse, SessionResponse, SessionStateResponse},
};

use crate::{AppState, client::stripe::SessionStatus};

/// Starts payment for a held booking by creating a Checkout Session.
///
/// 200 rather than 202: unlike the write endpoints elsewhere, the caller needs the
/// response *body* to proceed, and the client secret comes from Stripe rather than from
/// a projection it would have to wait for.
pub async fn create_session(
    AuthedJwt { user_id, .. }: AuthedJwt,
    State(state): State<AppState>,
    Valid(request): Valid<CreateSessionRequest>,
) -> MyResult<impl IntoResponse> {
    let session = state
        .payment_service
        .create_session(&user_id, request)
        .await?;

    Ok((
        StatusCode::OK,
        Json(SessionResponse {
            session_id: session.session_id,
            client_secret: session.client_secret,
        }),
    ))
}

/// What became of a checkout, for the screen the renter lands on.
///
/// Scoped to the caller's own payment row, and answers 404 rather than 403 for anyone
/// else's session — a 403 would confirm it exists.
pub async fn session_state(
    AuthedJwt { user_id, .. }: AuthedJwt,
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> MyResult<impl IntoResponse> {
    let (state, booking_id) = state
        .payment_service
        .session_state(&user_id, &session_id)
        .await?;

    Ok(Json(SessionStateResponse {
        status: match state.status {
            SessionStatus::Complete => "complete",
            SessionStatus::Open => "open",
            SessionStatus::Expired => "expired",
        },
        paid: state.paid,
        client_secret: state.client_secret,
        booking_id,
    }))
}

/// What this host has earned and what is withdrawable.
///
/// Scoped to the token's own user, with no id in the path — a host asking about someone
/// else's income is not a request this endpoint can express.
pub async fn earnings(
    AuthedJwt { user_id, .. }: AuthedJwt,
    State(state): State<AppState>,
) -> MyResult<impl IntoResponse> {
    let earnings = state.payment_service.earnings(&user_id).await?;

    Ok(Json(EarningsResponse {
        available_cents: earnings.available_cents(),
        earned_cents: earnings.earned_cents,
        paid_out_cents: earnings.paid_out_cents,
    }))
}

/// Withdraws the whole available balance. Nothing real moves.
///
/// No amount in the request: the server computes it, so there is no figure a client
/// could inflate. 202 with a `seq`, like every other write — the payout row appears
/// once the projector has applied the event.
pub async fn request_payout(
    AuthedJwt { user_id, .. }: AuthedJwt,
    State(state): State<AppState>,
) -> MyResult<impl IntoResponse> {
    let (token, amount_cents) = state.payment_service.request_payout(&user_id).await?;

    Ok((
        StatusCode::ACCEPTED,
        Json(PayoutResponse {
            seq: token,
            amount_cents,
        }),
    ))
}

/// `POST /internal/backfill` — re-emit every payout, for rebuilding a consumer.
///
/// Off the ingress and unauthenticated by construction; see the same handler in
/// user-service for why that is the whole of the access control.
///
/// Payouts only. `PaymentService::backfill` says why the payments themselves have
/// nothing downstream to rebuild.
pub async fn backfill(State(state): State<AppState>) -> MyResult<impl IntoResponse> {
    Ok(backfilled(state.payment_service.backfill().await?))
}
