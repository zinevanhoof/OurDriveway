//! Onboarding a host onto Stripe Connect, and answering whether they can be paid.
//!
//! Request-driven, so `*_service.rs` and not `*_worker_service.rs` — the withdraw
//! screen calls both of these directly.
//!
//! Nothing here publishes an event, and that is the deliberate difference from every
//! other service in this crate. A connected account is not a fact about *our* domain
//! that other services project; it is a handle to a third party, and it stays where the
//! Stripe SDK already is. There is nothing downstream to tell.

use std::sync::Arc;

use shared::error::myerror::{ContextExt, MyResult};
use uuid::Uuid;

use crate::{
    client::stripe::{AccountState, Stripe},
    repository::{
        connect_account_repository::ConnectAccountRepository,
        host_mirror_repository::HostMirrorRepository,
    },
};

pub struct ConnectService {
    /// The pool. See the note on `PaymentService::db` — the repositories are stateless.
    db: sqlx::PgPool,
    stripe: Arc<Stripe>,
}

/// Where a host is in onboarding, as the withdraw screen needs to hear it.
///
/// Three states because there are three different screens: an explanation and a button,
/// a "we are checking your details" notice, and the withdraw form itself.
pub enum ConnectStatus {
    /// No connected account, and no country on the profile to open one with.
    ///
    /// Its own state rather than a failure at the moment the host presses the button:
    /// Accounts v2 fixes `identity.country` permanently at creation, so it has to be
    /// right before anything is created, and the screen can say so up front instead of
    /// offering a button that 422s.
    NeedsCountry,
    /// No connected account at all. Nothing has been created at Stripe yet — that
    /// happens when the host asks, not when they look.
    None,
    /// An account exists but Stripe will not pay it: onboarding was abandoned halfway,
    /// or the details are submitted and under review.
    Onboarding,
    /// `payouts_enabled`. The only state in which a withdrawal may be offered.
    Enabled { bank_last4: Option<String> },
}

impl ConnectService {
    pub fn new(db: sqlx::PgPool, stripe: Arc<Stripe>) -> Self {
        Self { db, stripe }
    }

    /// Whether this host can be paid, asked of Stripe rather than of a column.
    ///
    /// One API call per withdraw-page load, and it buys the thing a cached flag cannot:
    /// an answer that is true *now*. A stale `enabled` shows a host a form Stripe then
    /// refuses; a stale `onboarding` hides a form from someone who finished five
    /// seconds ago in Stripe's own iframe, which is precisely when this is called.
    ///
    /// ponytail: an API call on every load. If it ever shows up in latency, the fix is
    /// `account.updated` in `stripe::verify` writing a cached flag onto
    /// `connect_account` — a webhook and a column, not a different design.
    pub async fn status(&self, owner_id: &Uuid) -> MyResult<ConnectStatus> {
        let Some(account_id) = ConnectAccountRepository::find(&self.db, owner_id).await? else {
            // Only asked when there is no account yet. Once one exists the country is
            // Stripe's and cannot be changed, so the profile's copy stops mattering.
            let has_country = HostMirrorRepository::find(&self.db, owner_id)
                .await?
                .is_some_and(|host| host.country.is_some());

            return Ok(if has_country {
                ConnectStatus::None
            } else {
                ConnectStatus::NeedsCountry
            });
        };

        let AccountState {
            payouts_enabled,
            bank_last4,
            ..
        } = self.stripe.retrieve_account(&account_id).await?;

        Ok(if payouts_enabled {
            ConnectStatus::Enabled { bank_last4 }
        } else {
            ConnectStatus::Onboarding
        })
    }

    /// The client secret the browser mounts Connect's embedded components against,
    /// creating the account first if this host has none.
    ///
    /// **Get-or-create on purpose.** The alternative is a separate "create account"
    /// endpoint the screen has to call first, which is one more round trip and one more
    /// state to be interrupted in — and a host who abandoned onboarding and came back
    /// would need the screen to know which of the two calls to make. Here the screen
    /// asks for the same thing every time.
    ///
    /// The account is created when a host asks to onboard, never when the page merely
    /// loads: `status` above deliberately does not do this.
    ///
    /// Two racing calls cannot leave two accounts pointed at: `Stripe::create_account`
    /// is idempotent per owner, and `ConnectAccountRepository::insert` answers with
    /// whichever row won.
    pub async fn account_session(&self, owner_id: &Uuid) -> MyResult<String> {
        let account_id = match ConnectAccountRepository::find(&self.db, owner_id).await? {
            Some(existing) => existing,
            None => {
                // Both required by Accounts v2 before a recipient configuration is
                // accepted, and neither is guessable: the country is immutable once the
                // account exists, so a default would cost a host their payouts rather
                // than merely being wrong.
                let host = HostMirrorRepository::find(&self.db, owner_id)
                    .await?
                    .context_not_found(("Not Found", "Could not find your account."))?;

                let country = host.country.as_deref().context_unprocessable_entity((
                    "Country Required",
                    "Add the country you bank in to your profile before setting up payouts.",
                ))?;

                let created = self
                    .stripe
                    .create_account(owner_id, &host.email, country)
                    .await?;
                tracing::info!(%owner_id, account_id = %created, "created a connected account");
                ConnectAccountRepository::insert(&self.db, owner_id, &created).await?
            }
        };

        self.stripe.account_session(&account_id).await
    }
}
