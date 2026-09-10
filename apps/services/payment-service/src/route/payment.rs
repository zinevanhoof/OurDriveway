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
    requests::payment::{CreateSessionRequest, PayoutRequest},
    responses::common::backfilled,
    responses::payment::{PayoutResponse, SessionResponse, SessionStateResponse},
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

// `GET /api/payment/earnings` was here. It is `GET /api/view/host/balance` now — reads
// are view-service's, and that one also answers what is still pending, which this
// service had no table to compute.
//
// The arithmetic did not move: `PaymentService::earnings_with` still runs inside
// `request_payout`'s transaction, below the advisory lock, and it is what decides how
// much a withdrawal actually pays out. What a host is *shown* may lag; what they are
// *paid* is computed here, from these tables, at the moment they ask for it.

/// Withdraws part or all of the caller's available balance, as a real Stripe Transfer.
///
/// The amount **is** in the request now, unlike `create_session` above, and the
/// difference is whose money it is: a renter choosing what to be charged would be
/// picking a price, a host choosing what to withdraw is picking how much of their own
/// balance to take. It is still not trusted — `request_payout` re-reads the balance
/// under an advisory lock and refuses anything larger, rather than clamping.
///
/// 202 with a `seq`, like every other write. The transfer itself has not happened when
/// this answers: the payout row lands as `requested`, and a worker turns it into
/// `paid` or `failed` a moment later.
pub async fn request_payout(
    AuthedJwt { user_id, .. }: AuthedJwt,
    State(state): State<AppState>,
    Valid(request): Valid<PayoutRequest>,
) -> MyResult<impl IntoResponse> {
    let (token, amount_cents) = state
        .payment_service
        .request_payout(&user_id, request.amount_cents)
        .await?;

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
