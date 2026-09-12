//! `id = caller` — everything about the caller as a person.
//!
//! Two routes, and the subject of both is the verified claim: there is no id to extract
//! and none to compare. `USERS_ME` and `PAYOUTS` each passed the reader's own id as a
//! variable, which was safe only because a table permission clause independently refused
//! everyone else's rows. With the clauses gone an id in a path would be a request rather
//! than a claim, so the namespace says `account` and the handler takes the id from the
//! token.
//!
//! The wallet takes the one parameter here that is not an identity — which month — and
//! that is exactly why it is safe as a query string.

use axum::{
    Json,
    extract::{Query, State},
};
use serde::Deserialize;
use shared::{
    error::myerror::MyResult,
    extractors::authed_jwt::AuthedJwt,
    responses::view::{AccountResponse, WalletResponse},
};

use crate::AppState;

/// `GET /api/view/account` — the caller's own profile, `email` included.
///
/// This endpoint predates the rest of the REST read API and existed because one
/// `FOR select` clause could not be both "only me" and "public": scoping `app_user` to
/// the caller broke spot-host names on the map, and leaving it world-readable meant
/// `users { id }` returned everyone. Choosing the row from the claim sidestepped it, and
/// the sidestep turned out to be the right shape — it is now one of four namespaces.
pub async fn account(
    AuthedJwt { user_id, .. }: AuthedJwt,
    State(state): State<AppState>,
) -> MyResult<Json<AccountResponse>> {
    Ok(Json(state.account_service.account(user_id).await?))
}

/// Query for [`wallet`]. Absent means the current month.
///
/// Optional rather than required so the first request needs to know nothing: a client
/// opening the wallet asks for `/account/wallet` and is told, in the response, which month
/// it got and which one to ask for next.
#[derive(Deserialize)]
pub struct WalletQuery {
    pub month: Option<String>,
}

/// `GET /api/view/account/wallet?month=YYYY-MM` — one month of everything that moved the
/// caller's money, newest first.
///
/// **A month is a page.** `nextMonth` in the response is the cursor: the next older month
/// that holds anything, or null at the end. A month that cannot be parsed is a 422 from
/// [`AccountService::wallet`], not from the extractor — `month` is a plain `Option<String>`
/// here because "which month" is one rule and it is stated once, next to the read.
///
/// [`AccountService::wallet`]: crate::service::account_service::AccountService::wallet
pub async fn wallet(
    AuthedJwt { user_id, .. }: AuthedJwt,
    State(state): State<AppState>,
    Query(q): Query<WalletQuery>,
) -> MyResult<Json<WalletResponse>> {
    Ok(Json(state.account_service.wallet(user_id, q.month).await?))
}
