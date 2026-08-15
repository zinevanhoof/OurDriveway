use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::Serialize;
use shared::{
    error::myerror::MyResult, events::STREAM_PAYMENTS, extract::Valid,
    extractors::authed_jwt::AuthedJwt, requests::payment::CreateSessionRequest,
};
use uuid::Uuid;

use crate::{AppState, service::stripe::SessionStatus};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionResponse {
    /// The handle the checkout screen navigates with. Everything it needs afterwards —
    /// the client secret, the booking, the outcome — it fetches back from this id, which
    /// is why it is the only thing that ever appears in a checkout URL.
    pub session_id: String,
    /// Saves the checkout screen an immediate round trip on the happy path. Not a secret
    /// in the bearer-token sense: it authorizes paying this one session and nothing else.
    pub client_secret: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionStateResponse {
    /// `complete`, `open` or `expired` — Stripe's own answer, not ours.
    pub status: &'static str,
    /// True when the money has actually arrived, as opposed to a `complete` session whose
    /// asynchronous method is still processing.
    pub paid: bool,
    /// Present while the session is still payable, so the screen can mount the Payment
    /// Element knowing nothing but the id in its URL.
    pub client_secret: Option<String>,
    /// So the screen can release the hold without the booking id ever being in the URL.
    pub booking_id: Uuid,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EarningsResponse {
    /// Withdrawable now: settled income minus what has already been taken out.
    pub available_cents: i64,
    /// Everything earned and settled, ever.
    pub earned_cents: i64,
    pub paid_out_cents: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PayoutResponse {
    pub seq: String,
    pub amount_cents: i64,
}

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
        .create_session(&request.booking_id, &user_id, &request.return_url)
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
        .session_state(&session_id, &user_id)
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
    let (seq, amount_cents) = state.payment_service.request_payout(&user_id).await?;

    Ok((
        StatusCode::ACCEPTED,
        Json(PayoutResponse {
            seq: format!("{STREAM_PAYMENTS}:{seq}"),
            amount_cents,
        }),
    ))
}
