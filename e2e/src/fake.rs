//! Stand-ins for the two providers the flows under test cannot run without.
//!
//! One axum server, pointed at by `LOCATIONIQ_BASE_URL` and `STRIPE_API_BASE`.
//!
//! **Why not `stripe-mock`.** It is stateless: every create answers the same fixture id,
//! and `payment_session UNIQUE (session_id)` refuses the second paying test. And a
//! fixture can never answer "paid", which `GET /api/payment/session/{id}` asks Stripe.
//! So the Stripe half here keeps sessions in a map and lets a test pay one.
//!
//! The bodies are the minimum async-stripe rc.8 will deserialize — its non-`Option`
//! fields and nothing else. A missing one surfaces as a 502 from payment-service with
//! the field named in its log.
//!
//! Connect is here too: Accounts v2 (hand-rolled JSON in payment-service, so only the
//! fields `retrieve_account` reads), the v1 account session, and transfers — which a test
//! can make Stripe refuse, the way it does when the platform balance is short.
//!
//! Resend is not faked: the stack runs with `NOTIFICATIONS_ENABLED=false`, which still
//! builds every mail and drops it at the send.

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
};

use axum::{
    Form, Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use serde_json::{Value, json};
use uuid::Uuid;

/// A spot whose address the fake refuses to geocode, so the "couldn't locate that
/// address" path has something to hit.
pub const UNKNOWN_ADDRESS: &str = "Nowhere";

#[derive(Clone)]
pub struct Fake {
    pub base: String,
    stripe: Arc<Mutex<Stripe>>,
}

#[derive(Default)]
struct Stripe {
    /// session id -> paid
    sessions: HashMap<String, bool>,
    expired: HashSet<String>,
    /// Intent ids a refund was issued against.
    refunded: HashSet<String>,
    /// Connected account id -> the host it was created for, from the `host_id`
    /// metadata payment-service stamps on it.
    accounts: HashMap<String, String>,
    /// Accounts whose onboarding is done: payouts active, a bank attached.
    onboarded: HashSet<String>,
    /// Accounts a transfer to is refused, as Stripe does when the platform balance
    /// cannot cover it.
    refusing: HashSet<String>,
    /// (destination account, amount) per transfer made.
    transfers: Vec<(String, i64)>,
    /// Destination account per transfer refused.
    refused: Vec<String>,
}

impl Stripe {
    fn account_of(&self, host_id: &str) -> Option<String> {
        self.accounts
            .iter()
            .find(|(_, host)| *host == host_id)
            .map(|(account, _)| account.clone())
    }
}

impl Fake {
    /// Binds an ephemeral port and serves on the calling runtime.
    pub async fn start() -> Fake {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind the fake");
        let fake = Fake {
            base: format!("http://{}", listener.local_addr().unwrap()),
            stripe: Arc::default(),
        };

        let app = Router::new()
            .route("/v1/search", get(search))
            .route("/v1/autocomplete", get(search))
            .route("/v1/checkout/sessions", post(create_session))
            .route("/v1/checkout/sessions/{id}", get(retrieve_session))
            .route("/v1/checkout/sessions/{id}/expire", post(expire_session))
            .route("/v1/refunds", post(refund))
            .route("/v2/core/accounts", post(create_account))
            .route("/v2/core/accounts/{id}", get(retrieve_account))
            .route("/v1/account_sessions", post(account_session))
            .route("/v1/transfers", post(transfer))
            .with_state(fake.clone());

        tokio::spawn(async move { axum::serve(listener, app).await });
        fake
    }

    /// What the renter's card does in the browser.
    pub fn pay(&self, session_id: &str) {
        self.stripe
            .lock()
            .unwrap()
            .sessions
            .insert(session_id.to_string(), true);
    }

    pub fn expired(&self, session_id: &str) -> bool {
        self.stripe.lock().unwrap().expired.contains(session_id)
    }

    pub fn refunded(&self, intent_id: &str) -> bool {
        self.stripe.lock().unwrap().refunded.contains(intent_id)
    }

    /// What the host does in Stripe's embedded onboarding. Panics if payment-service
    /// never created an account for them.
    pub fn onboard(&self, host_id: &Uuid) {
        let mut stripe = self.stripe.lock().unwrap();
        let account = stripe
            .account_of(&host_id.to_string())
            .expect("a connected account for the host");
        stripe.onboarded.insert(account);
    }

    pub fn refuse_transfers(&self, host_id: &Uuid, refuse: bool) {
        let mut stripe = self.stripe.lock().unwrap();
        let account = stripe
            .account_of(&host_id.to_string())
            .expect("a connected account for the host");
        if refuse {
            stripe.refusing.insert(account);
        } else {
            stripe.refusing.remove(&account);
        }
    }

    /// How many transfers to the host's account were refused.
    pub fn refusals_to(&self, host_id: &Uuid) -> usize {
        let stripe = self.stripe.lock().unwrap();
        let account = stripe.account_of(&host_id.to_string());
        stripe.refused.iter().filter(|to| Some(*to) == account.as_ref()).count()
    }

    /// Amounts transferred to the host's account, in order.
    pub fn transfers_to(&self, host_id: &Uuid) -> Vec<i64> {
        let stripe = self.stripe.lock().unwrap();
        let Some(account) = stripe.account_of(&host_id.to_string()) else {
            return vec![];
        };
        stripe
            .transfers
            .iter()
            .filter(|(to, _)| *to == account)
            .map(|(_, amount)| *amount)
            .collect()
    }
}

/// Brussels, whatever was asked — the spot's timezone then resolves to one with DST,
/// which is the case a wall-clock bug shows up in.
async fn search(Query(q): Query<HashMap<String, String>>) -> impl IntoResponse {
    if q.get("q").is_some_and(|q| q.contains(UNKNOWN_ADDRESS)) {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "Unable to geocode" })),
        );
    }
    (
        StatusCode::OK,
        Json(json!([{ "lat": "50.8466", "lon": "4.3528", "display_name": "Brussels" }])),
    )
}

async fn create_session(State(fake): State<Fake>) -> Json<Value> {
    let id = format!("cs_test_{}", Uuid::now_v7().simple());
    fake.stripe
        .lock()
        .unwrap()
        .sessions
        .insert(id.clone(), false);
    Json(session(&id, false, false))
}

async fn retrieve_session(State(fake): State<Fake>, Path(id): Path<String>) -> impl IntoResponse {
    let stripe = fake.stripe.lock().unwrap();
    match stripe.sessions.get(&id) {
        Some(&paid) => (
            StatusCode::OK,
            Json(session(&id, paid, stripe.expired.contains(&id))),
        ),
        None => (StatusCode::NOT_FOUND, Json(no_such(&id))),
    }
}

async fn expire_session(State(fake): State<Fake>, Path(id): Path<String>) -> impl IntoResponse {
    let mut stripe = fake.stripe.lock().unwrap();
    match stripe.sessions.get(&id).copied() {
        Some(paid) => {
            stripe.expired.insert(id.clone());
            (StatusCode::OK, Json(session(&id, paid, true)))
        }
        None => (StatusCode::NOT_FOUND, Json(no_such(&id))),
    }
}

async fn refund(State(fake): State<Fake>, Form(form): Form<HashMap<String, String>>) -> Json<Value> {
    let intent = form.get("payment_intent").cloned().unwrap_or_default();
    fake.stripe.lock().unwrap().refunded.insert(intent.clone());
    Json(json!({
        "id": format!("re_{}", Uuid::now_v7().simple()),
        "object": "refund",
        "amount": 0,
        "created": now(),
        "currency": "eur",
        "status": "succeeded",
        "payment_intent": intent,
    }))
}

/// Accounts v2. Keyed on the host like the real call's idempotency key, so a repeat
/// answers the same account.
async fn create_account(State(fake): State<Fake>, Json(body): Json<Value>) -> Json<Value> {
    let host_id = body["metadata"]["host_id"].as_str().unwrap_or_default().to_string();
    let mut stripe = fake.stripe.lock().unwrap();
    let id = stripe.account_of(&host_id).unwrap_or_else(|| {
        let id = format!("acct_{}", Uuid::now_v7().simple());
        stripe.accounts.insert(id.clone(), host_id);
        id
    });
    Json(account(&id, false))
}

async fn retrieve_account(State(fake): State<Fake>, Path(id): Path<String>) -> impl IntoResponse {
    let stripe = fake.stripe.lock().unwrap();
    if !stripe.accounts.contains_key(&id) {
        return (StatusCode::NOT_FOUND, Json(no_such(&id)));
    }
    (StatusCode::OK, Json(account(&id, stripe.onboarded.contains(&id))))
}

/// Only the parts `retrieve_account` reads: the payouts capability and the bank.
fn account(id: &str, onboarded: bool) -> Value {
    json!({
        "id": id,
        "object": "v2.core.account",
        "configuration": { "recipient": {
            "capabilities": { "stripe_balance": {
                "payouts": { "status": if onboarded { "active" } else { "pending" } },
            }},
            "default_outbound_destination": onboarded.then(|| json!({ "last4": "4321" })),
        }},
    })
}

async fn account_session(Form(form): Form<HashMap<String, String>>) -> Json<Value> {
    // Every component the type has, all off: its fields are required, and unknown
    // keys are ignored, so one `features` carrying every flag fits them all.
    let flags = [
        "capture_payments", "card_management", "card_spend_dispute_management",
        "cardholder_management", "destination_on_behalf_of_charge_management",
        "disable_stripe_user_authentication", "dispute_management", "edit_payout_schedule",
        "external_account_collection", "instant_payouts", "refund_management",
        "send_money", "smart_disputes_management", "spend_control_management",
        "standard_payouts", "transfer_balance",
    ];
    let off = json!({
        "enabled": false,
        "features": flags.iter().map(|f| (f.to_string(), json!(false))).collect::<serde_json::Map<_, _>>(),
    });
    let components: serde_json::Map<_, _> = [
        "account_management", "account_onboarding", "balance_report", "balances",
        "disputes_list", "documents", "financial_account", "financial_account_transactions",
        "instant_payouts_promotion", "issuing_card", "issuing_cards_list",
        "notification_banner", "payment_details", "payment_disputes", "payments",
        "payout_details", "payout_reconciliation_report", "payouts", "payouts_list",
        "tax_registrations", "tax_settings",
    ]
    .iter()
    .map(|c| (c.to_string(), off.clone()))
    .collect();

    let account = form.get("account").cloned().unwrap_or_default();
    Json(json!({
        "object": "account_session",
        "account": account,
        "client_secret": format!("accs_secret_{}", Uuid::now_v7().simple()),
        "components": components,
        "expires_at": now() + 3600,
        "livemode": false,
    }))
}

async fn transfer(
    State(fake): State<Fake>,
    Form(form): Form<HashMap<String, String>>,
) -> impl IntoResponse {
    let destination = form.get("destination").cloned().unwrap_or_default();
    let amount: i64 = form.get("amount").and_then(|a| a.parse().ok()).unwrap_or_default();

    let mut stripe = fake.stripe.lock().unwrap();
    if stripe.refusing.contains(&destination) {
        stripe.refused.push(destination);
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": {
                "type": "invalid_request_error",
                "message": "You have insufficient available funds in your Stripe account.",
            }})),
        );
    }
    stripe.transfers.push((destination.clone(), amount));

    let id = format!("tr_{}", Uuid::now_v7().simple());
    (
        StatusCode::OK,
        Json(json!({
            "id": id,
            "object": "transfer",
            "amount": amount,
            "amount_reversed": 0,
            "created": now(),
            "currency": "eur",
            "destination": destination,
            "livemode": false,
            "metadata": {},
            "reversals": { "object": "list", "data": [], "has_more": false, "url": format!("/v1/transfers/{id}/reversals") },
            "reversed": false,
        })),
    )
}

fn session(id: &str, paid: bool, expired: bool) -> Value {
    json!({
        "id": id,
        "object": "checkout.session",
        "automatic_tax": { "enabled": false },
        "client_secret": (!expired).then(|| format!("{id}_secret_e2e")),
        "created": now(),
        "custom_fields": [],
        "custom_text": {},
        "expires_at": now() + 3600,
        "livemode": false,
        "mode": "payment",
        "payment_method_types": ["card"],
        "payment_status": if paid { "paid" } else { "unpaid" },
        "shipping_options": [],
        "status": match (expired, paid) {
            (true, _) => "expired",
            (_, true) => "complete",
            _ => "open",
        },
    })
}

fn no_such(id: &str) -> Value {
    json!({ "error": { "type": "invalid_request_error", "message": format!("No such checkout.session: '{id}'") } })
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}
