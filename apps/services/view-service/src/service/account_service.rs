use chrono::Utc;
use shared::{
    error::myerror::{ContextExt, MyResult},
    responses::view::{AccountResponse, WalletResponse, WalletTransactionResponse},
};
use uuid::Uuid;

use crate::{
    policy,
    repository::{user_repository::ViewUserRepository, wallet_repository::WalletRepository},
    service::settled_before,
};

/// `id = caller` — everything about the caller as a person.
///
/// The subject of both reads is the verified claim, so neither takes an id to compare:
/// the caller's own `user_id` arrives from the route's `AuthedJwt` and is the whole
/// predicate. That is why this is the only namespace whose statements may hold
/// [`shared::projections::AccountProjection`]'s scoped columns.
pub struct AccountService {
    /// The pool, not a repository. The repositories are stateless — diesel's
    /// `AsyncPgConnection` is what a statement runs on, so there is nothing for a
    /// repository to hold.
    pub db: shared::db::Db,
}

impl AccountService {
    /// The caller's own profile, `email` included.
    ///
    /// `profile` is `None` while the caller's own `UserRegistered` is still in flight. The
    /// id comes from the claim regardless, so this never has to 404.
    pub async fn account(&self, user_id: Uuid) -> MyResult<AccountResponse> {
        let mut conn = shared::db::conn(&self.db).await?;

        let profile = ViewUserRepository::find_for_account(&mut conn, user_id).await?;

        Ok(AccountResponse {
            id: user_id,
            profile: profile.map(Into::into),
        })
    }

    /// One month of everything that moved the caller's money, newest first.
    ///
    /// Charges as a host, charges as a renter, refunds either way, and withdrawals — five
    /// sources in one ordered list, which is what a wallet is. `next_month` in the
    /// response is the cursor: the next older month that holds anything, or `None` at the
    /// end.
    ///
    /// **A month is a page.** Grouping by month is what the screen shows, so it is also
    /// how the history is fetched — no offset to drift, and every page carries the totals
    /// for exactly the rows in it.
    ///
    /// `month` absent means the current one, so the first request needs to know nothing: a
    /// client opens the wallet, and the response tells it which month it got and which one
    /// to ask for next.
    ///
    /// Three things are decided here rather than in SQL, and each for the same reason —
    /// two statements can disagree where one fold cannot:
    ///
    /// - `in_cents`/`out_cents` are folded over the projections this page will show.
    /// - `next_month` comes back as an instant and is labelled by the same function that
    ///   labelled `month`, so a page and the page it points at cannot be named by two
    ///   different rules.
    /// - `pending` compares the booking's end against the settlement cutoff, which the
    ///   statement has no other use for.
    pub async fn wallet(
        &self,
        user_id: Uuid,
        month: Option<String>,
    ) -> MyResult<WalletResponse> {
        let now = Utc::now();
        let month = month.unwrap_or_else(|| policy::wallet::label(now));

        let (start, end) = policy::wallet::bounds(&month)
            .context_bad_request(("Invalid month", "Ask for a month as YYYY-MM."))?;

        // One connection for both statements: the rows and the cursor over them should not
        // come back from two different pooled connections.
        let mut conn = shared::db::conn(&self.db).await?;

        let transactions =
            WalletRepository::find_month_for_account(&mut conn, user_id, start, end).await?;

        let (in_cents, out_cents) = policy::wallet::totals(&transactions);

        let next_month = WalletRepository::find_newest_before(&mut conn, user_id, start)
            .await?
            .map(policy::wallet::label);

        let settled = settled_before(now);

        Ok(WalletResponse {
            month,
            in_cents,
            out_cents,
            next_month,
            transactions: transactions
                .into_iter()
                .map(|t| {
                    let pending = policy::wallet::pending(t.settles_at, t.pending_now, settled);
                    WalletTransactionResponse::new(t, pending)
                })
                .collect(),
        })
    }
}
