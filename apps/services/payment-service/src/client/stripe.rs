//! The seam. Everything Stripe-shaped lives in this file.
//!
//! Nothing outside it names a Stripe type: these functions take and return `Uuid`,
//! `i64` cents and `&str` ids. That is deliberate — the crate is pinned to a release
//! candidate (see the comment in the workspace Cargo.toml), so when 1.0 lands, or if
//! this is ever swapped for hand-rolled HTTP, this is the only file that changes.

use std::collections::HashMap;

use chrono::{DateTime, Duration, Utc};

use shared::error::myerror::{ContextExt, MyError, MyResult};
// `StripeRequest` is imported for its `customize()` method, which is what carries an
// idempotency key onto a request — it is a trait method, not an inherent one.
use shared::{general_models::booking::Booked, rpc::spot::SpotCard};
use stripe::{Client, IdempotencyKey, RequestStrategy, StripeError, StripeRequest};
use stripe_checkout::checkout_session::{
    CreateCheckoutSession, CreateCheckoutSessionLineItems, CreateCheckoutSessionLineItemsPriceData,
    CreateCheckoutSessionPaymentIntentData, ExpireCheckoutSession, ProductData,
    RetrieveCheckoutSession,
};
// No `account` imports: creating and retrieving a connected account is Accounts **v2**,
// which this crate does not cover — see `Stripe::create_account`. What is left on v1 is
// only what has no v2 endpoint at all.
use stripe_connect::account_session::{
    AccountConfigParam, CreateAccountSession, CreateAccountSessionComponents,
};
use stripe_connect::transfer::CreateTransfer;
// These enums come from `stripe_shared` but are re-exported here, so that crate stays
// transitive rather than becoming another direct dependency for a handful of type names.
use stripe_checkout::{
    CheckoutSessionMode, CheckoutSessionPaymentStatus, CheckoutSessionStatus, CheckoutSessionUiMode,
};
use stripe_core::refund::CreateRefund;
use stripe_types::Currency;
use stripe_webhook::{EventObject, Webhook};
use uuid::Uuid;

/// The metadata key carrying our booking id.
///
/// Set on the session's **PaymentIntent**, not on the session. That is load-bearing:
/// session metadata is not copied onto the intent, and the event we confirm bookings on
/// is `payment_intent.succeeded`. Put it only on the session and every webhook arrives
/// with no booking id, `verify` returns `Ignored`, and paid bookings never confirm.
const BOOKING_ID_KEY: &str = "booking_id";

/// Our host id, set on the connected account. Never read back — the mapping this
/// service trusts is the `connect_account` row, and this exists so a human looking at a
/// Stripe dashboard can tell whose account they are looking at.
const HOST_ID_KEY: &str = "host_id";

/// Our payout id, set on the transfer. Same job, one row further down.
const PAYOUT_ID_KEY: &str = "payout_id";

/// How long past the hold's own expiry the Checkout Session stays alive.
///
/// Stripe requires `expires_at` to be 30 minutes to 24 hours from creation, which is
/// longer than the 15-minute `HOLD` in booking-service. That sounds like a conflict and
/// isn't: when the hold lapses, `Released` reaches `settle_up` and the session is expired
/// there, so the renter's form goes dead with the hold. This is only the backstop for the
/// case where that call never lands.
///
/// Measured from `hold_until` rather than from now, and that is **not** a detail. The
/// session is created under an idempotency key derived from the booking, so a resumed
/// checkout sends the same key again — and Stripe refuses a replay whose parameters
/// differ ("Keys for idempotent requests can only be used with the same parameters they
/// were first used with"). Anchoring to `hold_until`, which is fixed when the booking is
/// reserved, makes every later call byte-identical. `Utc::now()` here would break every
/// resume, which is exactly what it did.
const SESSION_GRACE_MINUTES: i64 = 30;

/// The Accounts v2 API version this service is written against.
///
/// Sent on every `/v2/…` request. v2 pins its shape to a dated version rather than to
/// the account's default, so this string is part of the contract the parsing below
/// assumes — change it and re-read `AccountState`.
const V2_VERSION: &str = "2026-08-26.dahlia";

const V2_ACCOUNTS: &str = "https://api.stripe.com/v2/core/accounts";

pub struct Stripe {
    client: Client,
    /// For the v2 calls, which `async-stripe` does not cover: the crate is generated
    /// from the v1 OpenAPI spec and has no `/v2/core/accounts` in it at all.
    ///
    /// Two clients in one struct is not duplication so much as the seam holding: both
    /// stay behind this file, and nothing outside it knows which API version answered.
    http: reqwest::Client,
    /// Held because the raw v2 requests have to authenticate themselves — `Client`
    /// owns its copy privately and exposes no way to borrow it.
    secret_key: String,
}

/// A freshly created Checkout Session. `client_secret` is the only part the browser sees.
pub struct NewSession {
    pub session_id: String,
    pub client_secret: String,
}

/// Where a session got to, as the checkout screen needs to hear it.
#[derive(Debug, PartialEq, Eq)]
pub enum SessionStatus {
    /// Paid. The booking confirms when the webhook lands, not because of this.
    Complete,
    /// Still payable — never attempted, or attempted and declined. The screen offers the
    /// Payment Element again on the same session.
    Open,
    /// Stripe voided it, or `settle_up` did when the hold lapsed. Nothing to retry.
    Expired,
}

pub struct SessionState {
    pub status: SessionStatus,
    /// `payment_status` is `paid`, as opposed to a session that is `complete` but whose
    /// asynchronous payment method is still processing.
    pub paid: bool,
    /// Handed back so the screen can mount the Payment Element from a session id alone,
    /// without the client having to have kept the secret across a redirect.
    pub client_secret: Option<String>,
}

// ─── the Accounts v2 wire shapes ────────────────────────────────────────────
//
// Hand-written because `async-stripe` has no v2 surface, and deliberately partial:
// every field this service does not read is left out rather than modelled, so a v2
// account gaining one is a field ignored and not a decode that fails. `Option`
// everywhere for the same reason — v2 answers `null` for anything `include` did not
// ask for, which is most of the object.

#[derive(serde::Deserialize)]
struct V2Account {
    id: String,
    configuration: Option<V2Configuration>,
}

#[derive(serde::Deserialize)]
struct V2Configuration {
    recipient: Option<V2Recipient>,
}

#[derive(serde::Deserialize)]
struct V2Recipient {
    capabilities: Option<V2Capabilities>,
    /// Where Stripe pays this account, once onboarding has added one. Kept as a raw
    /// value: it is only ever read for four digits to print, its shape varies by payout
    /// rail, and nothing here should fail to decode over a summary line.
    default_outbound_destination: Option<serde_json::Value>,
}

impl V2Recipient {
    /// The last four digits of the destination bank account, if Stripe named one.
    ///
    /// Searches the destination for a `last4` at either level, because a bank account
    /// may be the object itself or nested under its type. `None` whenever it is absent,
    /// which is every account that has not finished onboarding — the withdraw screen
    /// drops the line rather than inventing a placeholder.
    fn bank_last4(self) -> Option<String> {
        let destination = self.default_outbound_destination?;
        let last4 = destination
            .get("last4")
            .or_else(|| destination.get("bank_account")?.get("last4"))?;
        last4.as_str().map(str::to_string)
    }
}

#[derive(serde::Deserialize)]
struct V2Capabilities {
    stripe_balance: Option<V2StripeBalance>,
}

#[derive(serde::Deserialize)]
struct V2StripeBalance {
    /// Stripe can move this account's balance to their bank.
    payouts: Option<V2Capability>,
}

#[derive(serde::Deserialize)]
struct V2Capability {
    /// `active`, `restricted`, `pending`, `unsupported`, `disabled` — compared as a
    /// string rather than an enum so a status Stripe adds is "not active" instead of a
    /// decode failure in the middle of the payout gate.
    status: String,
}

#[derive(serde::Deserialize)]
struct V2Error {
    error: V2ErrorBody,
}

#[derive(serde::Deserialize)]
struct V2ErrorBody {
    message: String,
}

/// What Stripe says about a host's connected account, reduced to the two things the
/// withdraw screen actually asks.
///
/// Read live on every status call rather than cached in a column — see the note over
/// `connect_account` in `migrations/payment/0003`. The whole `Account` object is a
/// hundred fields of requirements and settings; none of the rest is our business.
pub struct AccountState {
    /// Stripe will pay this account: `configuration.recipient.capabilities
    /// .stripe_balance.payouts` is `active`. The **only** gate on showing the withdraw
    /// form.
    ///
    /// `details_submitted` used to sit beside this and is gone with the v1 object — v2
    /// has no such flag, and nothing read it: "an account that cannot be paid yet" is
    /// one screen whether the host abandoned onboarding or is still under review, and
    /// Stripe's own component explains which inside its iframe.
    pub payouts_enabled: bool,
    /// Last four of the bank account Stripe pays into, for the summary line. `None`
    /// whenever Stripe hands back no external account, which the screen renders by
    /// dropping the line rather than by inventing a placeholder.
    pub bank_last4: Option<String>,
}

/// What became of a Transfer.
///
/// The distinction is the worker's whole retry policy, so it is a return value rather
/// than an error: `Refused` is Stripe's own answer and will be the same answer forever
/// (`balance_insufficient`, an account that cannot receive transfers), so the payout is
/// marked failed and the money returns to the balance. A transport failure or a 5xx is
/// an `Err` instead, which NAKs and comes back — and the idempotency key means the
/// retry cannot pay twice.
pub enum Transferred {
    /// `tr_…`
    Ok(String),
    /// Stripe's own message, for the log.
    Refused(String),
}

/// What a verified webhook turned out to be about.
///
/// `Ignored` is not an error: a sandbox is shared, `--events` filtering is
/// best-effort, and an event for a booking or a type we don't handle is expected
/// traffic. The caller answers 200 to it so Stripe stops retrying.
pub enum Outcome {
    Succeeded { booking_id: Uuid, intent_id: String },
    Failed { booking_id: Uuid, reason: String },
    Ignored,
}

impl Stripe {
    pub fn new(secret_key: &str) -> Self {
        Self {
            client: Client::new(secret_key),
            http: reqwest::Client::new(),
            secret_key: secret_key.to_string(),
        }
    }

    /// Creates the Checkout Session a renter will pay.
    ///
    /// `amount_cents` comes from the booking as the server priced it at reserve time.
    /// No figure a client sent reaches here — that is the one parameter where it would
    /// matter, because it is what gets charged.
    ///
    /// `return_url` **is** the client's, and is the only client-supplied value on this
    /// call. It has to be: only the client knows whether it is the web build returning
    /// to its own origin or the Tauri build returning to a `ourdriveway://` deep link.
    /// Why that is not the open redirect it looks like is argued where the field is
    /// declared — see `shared::requests::payment::CreateSessionRequest`.
    ///
    /// The idempotency key is derived from the booking rather than random, which is
    /// what makes a double-submitted checkout safe: the second call before our own
    /// projection has landed returns Stripe's *first* session instead of creating a
    /// second one that could also be paid.
    pub async fn create_session(
        &self,
        booking_id: &Uuid,
        amount_cents: i64,
        booked: &Booked,
        card: Option<&SpotCard>,
        hold_until: DateTime<Utc>,
        return_url: &str,
    ) -> MyResult<NewSession> {
        // A booking is not a product, so one is described inline. `price_data` exists for
        // exactly this — there is no catalogue to point at and never will be, since every
        // booking is a different number of hours at a different spot's rate.
        //
        // This line item is the *whole* description of the purchase: the checkout screen
        // renders it from `getSession()` and never refetches anything, and it is what
        // lands on the renter's Stripe receipt.
        let line_item = CreateCheckoutSessionLineItems {
            quantity: Some(1),
            price_data: Some(CreateCheckoutSessionLineItemsPriceData {
                unit_amount: Some(amount_cents),
                product_data: Some(product(booking_id, booked, card)),
                ..CreateCheckoutSessionLineItemsPriceData::new(Currency::EUR)
            }),
            ..CreateCheckoutSessionLineItems::new()
        };

        // On the *intent*, not the session — see BOOKING_ID_KEY.
        let intent_data = CreateCheckoutSessionPaymentIntentData {
            metadata: Some(HashMap::from([(
                BOOKING_ID_KEY.to_string(),
                booking_id.to_string(),
            )])),
            description: Some(format!("OurDriveway booking {booking_id}")),
            ..CreateCheckoutSessionPaymentIntentData::new()
        };

        let session = CreateCheckoutSession::new()
            // `Elements` is what lets the Payment Element mount in our own page against
            // this session. `HostedPage` would send the renter to checkout.stripe.com.
            .ui_mode(CheckoutSessionUiMode::Elements)
            .mode(CheckoutSessionMode::Payment)
            .return_url(return_url.to_string())
            .expires_at((hold_until + Duration::minutes(SESSION_GRACE_MINUTES)).timestamp())
            .line_items(vec![line_item])
            .payment_intent_data(intent_data)
            .customize()
            .request_strategy(RequestStrategy::Idempotent(idempotency_key(
                "session", booking_id,
            )?))
            .send(&self.client)
            .await
            .map_err(|e| stripe_err("create checkout session", e))?;

        // Absent for ui_modes that don't confirm client-side, which is not how we create
        // them. An error rather than an unwrap: there is nothing the browser can do with
        // a missing secret.
        let client_secret = session.client_secret.ok_or_else(|| {
            MyError::Bus("stripe returned a session with no client_secret".to_string())
        })?;

        Ok(NewSession {
            session_id: session.id.as_str().to_string(),
            client_secret,
        })
    }

    /// Returns the whole amount. Keyed on the payment so a redelivered `Cancelled`
    /// cannot refund twice even if it beats our own projection.
    pub async fn refund(&self, intent_id: &str, payment_id: &Uuid) -> MyResult<String> {
        let refund = CreateRefund::new()
            .payment_intent(intent_id.to_string())
            .customize()
            .request_strategy(RequestStrategy::Idempotent(idempotency_key(
                "refund", payment_id,
            )?))
            .send(&self.client)
            .await
            .map_err(|e| stripe_err("create refund", e))?;

        Ok(refund.id.as_str().to_string())
    }

    /// Asks Stripe what became of a session.
    ///
    /// The checkout screen's whole source of truth after a redirect. `session.status` is
    /// Stripe's own answer and is available the instant the renter lands, which is why
    /// nothing here infers an outcome from our own projection — that lags the webhook,
    /// and "not confirmed yet" and "failed" look identical from the outside.
    ///
    /// `payment_intent` is expanded so a paid session can report the intent's status too,
    /// which is what distinguishes "paid" from "still processing" for the slower
    /// asynchronous methods.
    pub async fn retrieve_session(&self, session_id: &str) -> MyResult<SessionState> {
        let session = RetrieveCheckoutSession::new(session_id.to_string())
            .expand(vec!["payment_intent".to_string()])
            .send(&self.client)
            .await
            .map_err(|e| stripe_err("retrieve checkout session", e))?;

        Ok(SessionState {
            status: match session.status {
                Some(CheckoutSessionStatus::Complete) => SessionStatus::Complete,
                Some(CheckoutSessionStatus::Expired) => SessionStatus::Expired,
                // `Open` and anything unrecognised both mean "not paid, still payable as
                // far as we know" — the safe reading, since it only ever offers a retry.
                _ => SessionStatus::Open,
            },
            paid: matches!(
                session.payment_status,
                CheckoutSessionPaymentStatus::Paid
                    | CheckoutSessionPaymentStatus::NoPaymentRequired
            ),
            client_secret: session.client_secret,
        })
    }

    /// Voids a session nobody paid.
    ///
    /// This is what stops a renter completing a payment for a hold that has already
    /// lapsed — an expired session cannot be confirmed. Racing a confirmation is fine:
    /// one of the two loses at Stripe, and if the payment wins, `settle_up` refunds it
    /// on the other edge.
    ///
    /// Takes the session because this is the one path that must work before a payment
    /// exists, and until then the session id is the only handle there is.
    pub async fn expire_session(&self, session_id: &str) -> MyResult<()> {
        ExpireCheckoutSession::new(session_id.to_string())
            .send(&self.client)
            .await
            .map_err(|e| stripe_err("expire checkout session", e))?;
        Ok(())
    }

    // ─── Connect: the host's side of the money ──────────────────────────────
    //
    // Charges are unchanged by everything below. A renter still pays the platform, and
    // the split happens only when a host withdraws — Stripe calls this "separate
    // charges and transfers". The alternative, a destination charge that splits at
    // checkout, would move the settlement window and the balance into Stripe and leave
    // our own wallet projection describing money it no longer owns.

    /// Creates the connected account a host will be paid into.
    ///
    /// # Accounts v2, and why this one call is hand-rolled
    ///
    /// `POST /v2/core/accounts`, not `/v1/accounts`. Stripe refuses the v1 endpoint for
    /// new integrations outright:
    ///
    ///     Stripe no longer recommends Accounts v1 for new Connect integrations.
    ///     Create connected accounts with POST /v2/core/accounts instead.
    ///
    /// `async-stripe` is generated from the v1 OpenAPI spec and has no v2 surface at
    /// all, so this is `reqwest` and a JSON body rather than a builder. Everything
    /// Stripe-shaped still stops at this file, which is the rule that matters.
    ///
    /// # The shape
    ///
    /// A **recipient** configuration: it is what lets an account receive funds from the
    /// platform, and requesting `stripe_balance.stripe_transfers` grants
    /// `stripe_balance.payouts` alongside it — receiving our transfer and being paid out
    /// to a bank, which is the whole of what a host needs. No `merchant` configuration:
    /// nothing is ever charged on this account.
    ///
    /// `dashboard: express` fixes the two responsibilities, and it is not a choice:
    ///
    ///     If `dashboard` is `express`, `fees_collector` must be `application` and
    ///     `losses_collector` must be `application`.
    ///
    /// The Express dashboard means the platform owns the relationship with the host, so
    /// the platform carries the fees and the negative balances.
    ///
    /// `contact_email` and `identity.country` are both **required** before a recipient
    /// configuration is accepted, which is why this takes them as arguments — see the
    /// `host` mirror in `migrations/payment/0004`. The country is immutable after
    /// creation, so it is asked of the host rather than guessed.
    ///
    /// # The `v4` in the idempotency key, and why it must be bumped
    ///
    /// **Stripe stores a failed request against its idempotency key**, parameters and
    /// all. A request rejected for a bad parameter poisons that key: fixing the
    /// parameter and retrying answers
    ///
    ///     Keys for idempotent requests can only be used with the same parameters they
    ///     were first used with.
    ///
    /// for the next 24 hours — a host locked out of onboarding by a bug that is already
    /// fixed. The key therefore carries a version of the *request shape*, not just the
    /// host. Change any field in the body below and bump it, or every host who already
    /// tried waits a day. It has been bumped for a wrong `losses` value, for a platform
    /// that had not enabled Connect yet, and now for the move to v2.
    pub async fn create_account(
        &self,
        host_id: &Uuid,
        email: &str,
        country: &str,
    ) -> MyResult<String> {
        let body = serde_json::json!({
            "contact_email": email,
            "identity": { "country": country },
            "dashboard": "express",
            "configuration": {
                "recipient": {
                    "capabilities": {
                        "stripe_balance": { "stripe_transfers": { "requested": true } }
                    }
                }
            },
            "defaults": {
                "responsibilities": {
                    "fees_collector": "application",
                    "losses_collector": "application"
                }
            },
            // Our id on their object, so a Stripe dashboard row can be traced back to a
            // host without a lookup here. The reverse direction is `connect_account`.
            "metadata": { HOST_ID_KEY: host_id.to_string() },
        });

        let account: V2Account = self
            .v2(
                self.http
                    .post(V2_ACCOUNTS)
                    .header("Idempotency-Key", format!("connect:v4:{host_id}"))
                    .json(&body),
                "create connected account",
            )
            .await?;

        Ok(account.id)
    }

    /// The short-lived secret the browser mounts Connect's embedded components against.
    ///
    /// Still `/v1/account_sessions`, and that is not a leftover: **there is no v2
    /// account-sessions endpoint**. Stripe's own guidance is that a v2 account id may be
    /// passed to a v1 endpoint, and this is the case it was written for — verified
    /// against a v2 account, which answers with a client secret exactly as a v1 one did.
    ///
    /// Two components, and they are the two halves of one job: `account_onboarding`
    /// collects identity and a bank account the first time, `account_management` lets a
    /// host change that bank account later. Without the second, changing a bank means
    /// re-running onboarding.
    ///
    /// Deliberately **not** idempotent. A session expires, and the frontend's
    /// `fetchClientSecret` is called again precisely to get a fresh one — replaying the
    /// first would hand back an expired secret forever.
    pub async fn account_session(&self, account_id: &str) -> MyResult<String> {
        let session = CreateAccountSession::new(
            account_id.to_string(),
            CreateAccountSessionComponents {
                account_onboarding: Some(AccountConfigParam::new(true)),
                account_management: Some(AccountConfigParam::new(true)),
                ..CreateAccountSessionComponents::new()
            },
        )
        .send(&self.client)
        .await
        .map_err(|e| stripe_err("create account session", e))?;

        Ok(session.client_secret)
    }

    /// Whether this host can be paid, and where.
    ///
    /// `include` is not optional politeness: **a v2 account answers `null` for
    /// everything not asked for**, so without it the configuration comes back empty and
    /// every host reads as un-onboarded.
    ///
    /// The gate is the *payouts* capability rather than the transfers one, and the
    /// difference is real. `stripe_transfers` says our Transfer into their balance will
    /// land; `payouts` says Stripe can then move it to their bank. A host with the
    /// first and not the second would take money out of our balance and watch it sit in
    /// theirs — so both must be active, and requiring `payouts` implies both, because
    /// Stripe grants the pair together.
    pub async fn retrieve_account(&self, account_id: &str) -> MyResult<AccountState> {
        // `include[0]`, not `include[]`. The v2 API rejects the bracket-array syntax
        // that v1 accepts, by name:
        //
        //     Query parameters with the [] array syntax are unsupported. Please
        //     provide exact indexes, i.e. value[0], value[1], etc.
        let url = format!("{V2_ACCOUNTS}/{account_id}?include[0]=configuration.recipient");
        let account: V2Account = self
            .v2(self.http.get(url), "retrieve connected account")
            .await?;

        let recipient = account.configuration.and_then(|c| c.recipient);
        let capabilities = recipient
            .as_ref()
            .and_then(|r| r.capabilities.as_ref())
            .and_then(|c| c.stripe_balance.as_ref());

        Ok(AccountState {
            // Absent reads as false throughout: a missing capability is not permission.
            payouts_enabled: capabilities
                .and_then(|b| b.payouts.as_ref())
                .is_some_and(|c| c.status == "active"),
            bank_last4: recipient.and_then(|r| r.bank_last4()),
        })
    }

    /// Sends one v2 request and decodes it, with the same error handling as every v1
    /// call in this file.
    ///
    /// The authentication, the pinned API version and the refusal-to-`MyError` mapping
    /// are identical for every v2 endpoint, and there is no builder generating them —
    /// so they live here once rather than at each call site.
    async fn v2<T: serde::de::DeserializeOwned>(
        &self,
        request: reqwest::RequestBuilder,
        what: &str,
    ) -> MyResult<T> {
        let response = request
            .bearer_auth(&self.secret_key)
            .header("Stripe-Version", V2_VERSION)
            .send()
            .await
            .map_err(|e| transport_err(what, e))?;

        let status = response.status();
        let body = response.text().await.map_err(|e| transport_err(what, e))?;

        if !status.is_success() {
            // Stripe's own words, the way `stripe_err` logs them for v1 — the message
            // names the parameter it refused and is the only useful thing in the body.
            let message = serde_json::from_str::<V2Error>(&body)
                .ok()
                .map(|e| e.error.message)
                .unwrap_or_else(|| body.clone());
            tracing::error!(%status, %message, "stripe: {what} was refused");

            return Err(MyError::api(
                axum::http::StatusCode::BAD_GATEWAY,
                "Payment Provider Unavailable",
                "Could not reach the payment provider. Please try again.",
            ));
        }

        serde_json::from_str(&body).map_err(|e| {
            tracing::error!(error = %e, "stripe: could not decode the {what} response");
            MyError::Bus(format!("decode stripe v2 response for {what}"))
        })
    }

    /// Moves money from the platform balance into a host's connected account.
    ///
    /// `/v1/transfers`, and like the account session above that is the only endpoint
    /// there is: the recipient configuration's `stripe_transfers` capability exists
    /// precisely to make this call work for a v2 account. (`/v2/money_management` is
    /// Global Payouts, a different product.)
    ///
    /// **The idempotency key is the whole safety of this path.** It is derived from the
    /// payout id, so a redelivered `PayoutRequested` — or a retry after we crashed
    /// between Stripe answering and our own commit — returns Stripe's first transfer
    /// rather than making a second one. The row's `status` guard is the cheaper check;
    /// this is the one that holds when the row has not been written yet.
    ///
    /// A refusal comes back as `Refused` rather than `Err`; see [`Transferred`].
    pub async fn transfer(
        &self,
        account_id: &str,
        amount_cents: i64,
        payout_id: &Uuid,
    ) -> MyResult<Transferred> {
        let result = CreateTransfer::new(Currency::EUR, account_id.to_string())
            .amount(amount_cents)
            .description(format!("OurDriveway payout {payout_id}"))
            .metadata(HashMap::from([(
                PAYOUT_ID_KEY.to_string(),
                payout_id.to_string(),
            )]))
            .customize()
            .request_strategy(RequestStrategy::Idempotent(idempotency_key(
                "payout", payout_id,
            )?))
            .send(&self.client)
            .await;

        match result {
            Ok(transfer) => Ok(Transferred::Ok(transfer.id.as_str().to_string())),

            // Stripe answered, and answered with a client error: not enough in the
            // platform balance, an account that cannot receive transfers, a currency it
            // will not convert. Retrying sends the identical request and gets the
            // identical refusal, so this is terminal.
            Err(StripeError::Stripe(api, status)) if status < 500 => {
                let reason = api
                    .message
                    .clone()
                    .unwrap_or_else(|| format!("stripe refused the transfer ({status})"));
                tracing::warn!(%payout_id, %status, %reason, "stripe refused a transfer");
                Ok(Transferred::Refused(reason))
            }

            // A 5xx, a timeout, a dropped connection, an unparseable body. Nothing is
            // known about whether the transfer happened, which is exactly the case the
            // idempotency key exists for — so this retries.
            Err(e) => Err(stripe_err("create transfer", e)),
        }
    }
}

/// Verifies a webhook's signature and says what it means.
///
/// `Err` here means the request did not come from Stripe — a bad signature, a missing
/// or malformed header, or a timestamp outside the tolerance window (replay). The
/// caller must answer 401, never 200.
///
/// A pure function taking the secret as an argument so it can be tested without a
/// client, a network or a config.
pub fn verify(payload: &str, signature: &str, secret: &str) -> MyResult<Outcome> {
    // Deliberately terse: the detail goes to the log, not to the caller, because
    // whoever is failing verification is not entitled to know why.
    let event = Webhook::construct_event(payload, signature, secret)
        .inspect_err(|e| tracing::warn!(error = %e, "rejected a webhook"))
        .context_unauthorized(("Unauthorized", "Signature verification failed."))?;

    Ok(match event.data.object {
        EventObject::PaymentIntentSucceeded(intent) => match booking_id_of(&intent.metadata) {
            Some(booking_id) => Outcome::Succeeded {
                booking_id,
                intent_id: intent.id.as_str().to_string(),
            },
            None => Outcome::Ignored,
        },

        EventObject::PaymentIntentPaymentFailed(intent) => match booking_id_of(&intent.metadata) {
            Some(booking_id) => Outcome::Failed {
                booking_id,
                reason: intent
                    .last_payment_error
                    .as_ref()
                    .and_then(|e| e.message.clone())
                    .unwrap_or_else(|| "no reason given".to_string()),
            },
            None => Outcome::Ignored,
        },

        // Everything else. A shared sandbox delivers other people's events and other
        // types; that is traffic, not a problem.
        _ => Outcome::Ignored,
    })
}

/// Stripe's cap on `images`, documented on the field. Sending a ninth is an API error, so
/// the list is truncated rather than trusted — a host can add photos after the session
/// exists, and a spot with nine of them must not make a checkout unpayable.
const MAX_IMAGES: usize = 8;

/// The line item: everything the renter sees about what they are buying.
///
/// The whole point of asking spot-service for a [`SpotCard`] — with it, the screen can
/// show the place, the times, the address and a photo without a single request of its own.
///
/// Without it, the booking id takes the place of a title. Deliberately not `describe()`:
/// if the spot cannot be named, the id is the only thing that identifies this purchase in
/// a dashboard, a dispute or a receipt, and a line reading only "Parking · 14 Aug" names
/// nothing at all. The times still appear underneath, because `booked` is ours and cannot
/// go missing.
///
/// **Must be deterministic**, like everything else under the idempotency key. It is —
/// given the same card. What makes that safe is that a card is only ever fetched for a
/// booking with no payment row yet; a resume returns the stored session without coming
/// anywhere near here. See `create_session` in payment_service.rs.
fn product(booking_id: &Uuid, booked: &Booked, card: Option<&SpotCard>) -> ProductData {
    let Some(card) = card else {
        return ProductData {
            description: Some(describe(booked)),
            ..ProductData::new(format!("Booking {booking_id}"))
        };
    };

    ProductData {
        description: Some(format!("{} · {}", describe(booked), card.address)),
        // Absolute URLs, straight from the projection — see `shared::media`.
        //
        // They have to be absolute, because **Stripe fetches these server-side and
        // re-hosts the image on its own CDN**. A bare key gives it nothing to fetch, so
        // the value is accepted without complaint, handed back verbatim, and the photo
        // simply never appears — which looks like it works right up until the checkout
        // screen renders nothing. Joining the hostname on at the edge is what this used
        // to do instead; storing it absolute removes the step.
        //
        // Two consequences worth carrying. `MEDIA_BASE` must be reachable FROM STRIPE,
        // not merely from a phone — a LAN address or a private bucket fails silently,
        // with no error on any request we make. And the image a session shows is frozen
        // at creation, because it is Stripe's copy: editing the spot's photos afterwards
        // does not change it.
        //
        // `None` rather than an empty array when a spot has no photos: nothing to say is
        // not the same as saying nothing, and it keeps the request shape honest.
        images: (!card.images.is_empty())
            .then(|| card.images.iter().take(MAX_IMAGES).cloned().collect()),
        ..ProductData::new(card.title.clone())
    }
}

/// When the renter is parked, in one line: `"Parking · 14 Aug, 09:00–11:00"`.
///
/// **Must be deterministic.** `Booked` is a `HashMap`, so the dates and slots are sorted
/// rather than iterated: `create_session` is called under an idempotency key derived from
/// the booking, and Stripe refuses a retry whose parameters differ from the first call.
/// An unsorted description would make a retried checkout fail with
/// "Keys for idempotent requests can only be used with the same parameters".
///
/// Times are bare wall-clock strings in the spot's zone and are rendered literally — no
/// timezone maths, and none needed. It is the local time the renter chose.
fn describe(booked: &Booked) -> String {
    let mut dates: Vec<&String> = booked.keys().collect();
    dates.sort();

    let total: usize = booked.values().map(|slots| slots.len()).sum();

    let Some(first) = dates.first() else {
        return "Parking".to_string();
    };
    let mut slots = booked.get(*first).cloned().unwrap_or_default();
    slots.sort_by(|a, b| a.start.cmp(&b.start));

    let Some(slot) = slots.first() else {
        return format!("Parking · {}", pretty_date(first));
    };

    let head = format!(
        "Parking · {}, {}–{}",
        pretty_date(first),
        slot.start,
        slot.end
    );

    // One line has to stand for the whole booking, so the rest are counted rather than
    // listed — Stripe truncates a long product name and a wall of times helps nobody.
    match total {
        0 | 1 => head,
        n => format!("{head} +{} more", n - 1),
    }
}

/// `"2026-08-14"` -> `"14 Aug"`, falling back to the raw string if it isn't that shape.
///
/// Hand-rolled rather than `chrono`: these are dates with no time and no zone, and parsing
/// them into a `DateTime` only to format them back would invite exactly the timezone
/// question this service is built to avoid.
fn pretty_date(date: &str) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];

    let parts: Vec<&str> = date.split('-').collect();
    let [_, month, day] = parts.as_slice() else {
        return date.to_string();
    };

    match month.parse::<usize>() {
        Ok(m) if (1..=12).contains(&m) => {
            format!("{} {}", day.trim_start_matches('0'), MONTHS[m - 1])
        }
        _ => date.to_string(),
    }
}

/// Our booking id off an intent's metadata, if it has one we recognise.
///
/// `None` covers both "not ours" (an intent created by another project against the
/// same sandbox) and "unparseable", which are handled identically: ignore it.
fn booking_id_of(metadata: &HashMap<String, String>) -> Option<Uuid> {
    metadata.get(BOOKING_ID_KEY)?.parse().ok()
}

/// `IdempotencyKey::new` rejects empty and over-255-character keys. Neither is
/// reachable from a uuid and a fixed prefix, so this failing is a programming error
/// rather than something a caller can act on.
fn idempotency_key(prefix: &str, id: &Uuid) -> MyResult<IdempotencyKey> {
    IdempotencyKey::new(format!("{prefix}:{id}"))
        .map_err(|e| MyError::Bus(format!("idempotency key: {e}")))
}

/// A v2 call that never reached Stripe, or whose answer could not be read off the
/// socket. The v1 half of this is `StripeError::ClientError`, handled by the fallback
/// arm of [`stripe_err`].
fn transport_err(what: &str, e: reqwest::Error) -> MyError {
    tracing::error!(error = %e, "stripe: {what} failed");
    MyError::api(
        axum::http::StatusCode::BAD_GATEWAY,
        "Payment Provider Unavailable",
        "Could not reach the payment provider. Please try again.",
    )
}

fn stripe_err(what: &str, e: StripeError) -> MyError {
    // Stripe's own words, pulled out field by field rather than left to the error's
    // `Display`.
    //
    // That `Display` formats the payload with `{:#?}`, and `redact-generated-debug` —
    // on for every Stripe crate here, so a stray `debug!` cannot print card or customer
    // detail — reduces that to a literal `ApiErrors { .. }`. The status code survives
    // and nothing else does, which is how a 400 saying "with a dashboard type of
    // `express`, the Connect application must control losses" reached the log as no
    // information at all.
    //
    // `message`, `code` and `param` are safe to keep: they describe *our* request, not
    // anyone's card. The redaction is still doing its job on every other field.
    match &e {
        StripeError::Stripe(api, status) => tracing::error!(
            %status,
            message = api.message.as_deref().unwrap_or("none"),
            code = ?api.code,
            param = api.param.as_deref().unwrap_or("none"),
            "stripe: {what} was refused"
        ),
        _ => tracing::error!(error = %e, "stripe: {what} failed"),
    }

    // 502, not 500: the failure is upstream. The detail stays in the log — a Stripe
    // error can name an intent id and a decline reason, neither of which belongs in a
    // response body.
    MyError::api(
        axum::http::StatusCode::BAD_GATEWAY,
        "Payment Provider Unavailable",
        "Could not reach the payment provider. Please try again.",
    )
}

#[cfg(test)]
mod tests {
    use shared::general_models::spot::TimeSlot;

    use super::*;

    const SECRET: &str = "whsec_test_secret";

    /// A minimal `payment_intent.succeeded` body. Only the fields `verify` reads have
    /// to be right; the rest of a real Stripe payload is noise here.
    fn payload(booking_id: &str) -> String {
        format!(
            r#"{{
                "id": "evt_test",
                "object": "event",
                "api_version": "2020-08-27",
                "created": 1492774577,
                "livemode": false,
                "pending_webhooks": 1,
                "type": "payment_intent.succeeded",
                "data": {{
                    "object": {{
                        "id": "pi_test123",
                        "object": "payment_intent",
                        "amount": 1500,
                        "currency": "eur",
                        "status": "succeeded",
                        "metadata": {{ "booking_id": "{booking_id}" }},
                        "capture_method": "automatic",
                        "confirmation_method": "automatic",
                        "created": 1492774577,
                        "livemode": false,
                        "payment_method_types": ["card"],
                        "amount_capturable": 0,
                        "amount_received": 1500
                    }}
                }}
            }}"#
        )
    }

    #[test]
    fn a_valid_signature_yields_the_booking_id() {
        let booking_id = Uuid::now_v7();
        let body = payload(&booking_id.to_string());
        let sig = Webhook::generate_test_header(&body, SECRET, None);

        match verify(&body, &sig, SECRET).expect("should verify") {
            Outcome::Succeeded {
                booking_id: got,
                intent_id,
            } => {
                assert_eq!(got, booking_id);
                assert_eq!(intent_id, "pi_test123");
            }
            _ => panic!("expected Succeeded"),
        }
    }

    /// The property that matters most: a body edited after signing must not verify.
    /// Without this, anyone who can POST to the webhook can confirm any booking.
    #[test]
    fn a_tampered_body_is_rejected() {
        let body = payload(&Uuid::now_v7().to_string());
        let sig = Webhook::generate_test_header(&body, SECRET, None);
        let tampered = body.replace("1500", "1");

        assert!(
            verify(&tampered, &sig, SECRET).is_err(),
            "a modified payload must not pass verification"
        );
    }

    #[test]
    fn the_wrong_secret_is_rejected() {
        let body = payload(&Uuid::now_v7().to_string());
        let sig = Webhook::generate_test_header(&body, SECRET, None);

        assert!(verify(&body, &sig, "whsec_someone_elses").is_err());
    }

    /// Replay protection. A signature stays cryptographically valid forever, so
    /// without a timestamp tolerance a captured request could be replayed at will —
    /// which for `payment_intent.succeeded` means re-confirming a cancelled booking.
    #[test]
    fn a_stale_timestamp_is_rejected() {
        let body = payload(&Uuid::now_v7().to_string());
        let ancient = chrono::Utc::now().timestamp() - 60 * 60 * 24;
        let sig = Webhook::generate_test_header(&body, SECRET, Some(ancient));

        assert!(
            verify(&body, &sig, SECRET).is_err(),
            "a signature from a day ago must not still be accepted"
        );
    }

    fn slot(start: &str, end: &str) -> TimeSlot {
        TimeSlot {
            start: start.into(),
            end: end.into(),
        }
    }

    #[test]
    fn one_slot_reads_as_a_date_and_a_range() {
        let booked = one_slot();
        assert_eq!(describe(&booked), "Parking · 14 Aug, 09:00–11:00");
    }

    /// The property Stripe's idempotency depends on: same booking, same string, every
    /// time. `Booked` is a HashMap, so without sorting this would vary per call and a
    /// retried checkout would be refused for differing parameters.
    #[test]
    fn the_description_is_deterministic_and_starts_at_the_earliest_slot() {
        let booked: Booked = HashMap::from([
            (
                "2026-08-15".to_string(),
                vec![slot("14:00", "15:00"), slot("08:00", "09:00")],
            ),
            ("2026-08-14".to_string(), vec![slot("09:00", "11:00")]),
        ])
        .into();

        let once = describe(&booked);
        for _ in 0..50 {
            assert_eq!(
                describe(&booked),
                once,
                "description must not vary per call"
            );
        }
        assert_eq!(once, "Parking · 14 Aug, 09:00–11:00 +2 more");
    }

    #[test]
    fn a_bare_date_survives_an_unexpected_shape() {
        assert_eq!(pretty_date("2026-08-14"), "14 Aug");
        assert_eq!(pretty_date("2026-13-14"), "2026-13-14");
        assert_eq!(pretty_date("nonsense"), "nonsense");
        // Never empty: an intent with no slots would otherwise get a blank product name,
        // which Stripe rejects.
        assert_eq!(describe(&Booked::new()), "Parking");
    }

    fn card(images: usize) -> SpotCard {
        SpotCard {
            title: "Kerkstraat 12".to_string(),
            address: "2000 Antwerpen".to_string(),
            images: (0..images).map(|i| format!("spots/{i}.jpeg")).collect(),
        }
    }

    fn one_slot() -> Booked {
        HashMap::from([("2026-08-14".to_string(), vec![slot("09:00", "11:00")])]).into()
    }

    #[test]
    fn a_card_puts_the_spot_on_the_line_item() {
        let p = product(&Uuid::nil(), &one_slot(), Some(&card(2)));

        assert_eq!(p.name, "Kerkstraat 12");
        assert_eq!(
            p.description.as_deref(),
            Some("Parking · 14 Aug, 09:00–11:00 · 2000 Antwerpen")
        );
        // Bare keys, untouched. A hostname appearing here means someone resolved them
        // server-side, which is the thing shared::media exists to prevent.
        assert_eq!(
            p.images.as_deref(),
            Some(["spots/0.jpeg".to_string(), "spots/1.jpeg".to_string()].as_slice())
        );
    }

    /// The property that makes the lookup safe to lose: spot-service being down costs
    /// the renter a title, never a payment.
    #[test]
    fn without_a_card_the_booking_id_names_it_and_the_times_survive() {
        let booking = Uuid::now_v7();
        let p = product(&booking, &one_slot(), None);

        assert_eq!(p.name, format!("Booking {booking}"));
        assert_eq!(
            p.description.as_deref(),
            Some("Parking · 14 Aug, 09:00–11:00")
        );
        assert!(p.images.is_none());
    }

    #[test]
    fn a_ninth_photo_cannot_break_a_checkout() {
        let p = product(&Uuid::nil(), &one_slot(), Some(&card(12)));
        assert_eq!(p.images.map(|i| i.len()), Some(MAX_IMAGES));

        // Absent, not empty: the two are different requests to Stripe.
        assert!(
            product(&Uuid::nil(), &one_slot(), Some(&card(0)))
                .images
                .is_none()
        );
    }

    /// Same input, same bytes — the line item goes to Stripe under an idempotency key
    /// derived from the booking, and a retry whose parameters differ is refused outright.
    #[test]
    fn the_line_item_is_deterministic() {
        let (booking, booked, card) = (Uuid::now_v7(), one_slot(), card(3));

        let once = product(&booking, &booked, Some(&card));
        for _ in 0..50 {
            let again = product(&booking, &booked, Some(&card));
            assert_eq!(again.name, once.name);
            assert_eq!(again.description, once.description);
            assert_eq!(again.images, once.images);
        }
    }

    /// An intent from another project sharing the sandbox. Verifies fine, means
    /// nothing to us, and must not be treated as a failure.
    #[test]
    fn an_intent_without_our_metadata_is_ignored() {
        let body = payload("not-a-uuid");
        let sig = Webhook::generate_test_header(&body, SECRET, None);

        assert!(matches!(
            verify(&body, &sig, SECRET).expect("should verify"),
            Outcome::Ignored
        ));
    }
}
