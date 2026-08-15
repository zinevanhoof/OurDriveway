//! The seam. Everything Stripe-shaped lives in this file.
//!
//! Nothing outside it names a Stripe type: these functions take and return `Uuid`,
//! `i64` cents and `&str` ids. That is deliberate — the crate is pinned to a release
//! candidate (see the comment in the workspace Cargo.toml), so when 1.0 lands, or if
//! this is ever swapped for hand-rolled HTTP, this is the only file that changes.

use std::collections::HashMap;

use chrono::{DateTime, Duration, Utc};

use shared::error::myerror::{MyError, MyResult};
// `StripeRequest` is imported for its `customize()` method, which is what carries an
// idempotency key onto a request — it is a trait method, not an inherent one.
use shared::{general_models::booking::Booked, rpc::spot::SpotCard};
use stripe::{Client, IdempotencyKey, RequestStrategy, StripeRequest};
use stripe_checkout::checkout_session::{
    CreateCheckoutSession, CreateCheckoutSessionLineItems, CreateCheckoutSessionLineItemsPriceData,
    CreateCheckoutSessionPaymentIntentData, ExpireCheckoutSession, ProductData,
    RetrieveCheckoutSession,
};
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

pub struct Stripe {
    client: Client,
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
        }
    }

    /// Creates the Checkout Session a renter will pay.
    ///
    /// `amount_cents` comes from the booking as the server priced it; nothing a client
    /// sent reaches here — and `return_url` comes from config rather than the request,
    /// because a client-supplied redirect target is an open redirect.
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
    let event = Webhook::construct_event(payload, signature, secret).map_err(|e| {
        // Deliberately terse: the detail goes to the log, not to the caller, because
        // whoever is failing verification is not entitled to know why.
        tracing::warn!(error = %e, "rejected a webhook");
        MyError::unauthorized("Unauthorized", "Signature verification failed.")
    })?;

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

fn stripe_err(what: &str, e: impl std::fmt::Display) -> MyError {
    // 502, not 500: the failure is upstream. Detail stays in the log — a Stripe error
    // can name an intent id and a decline reason, neither of which belongs in a
    // response body.
    tracing::error!(error = %e, "stripe: {what} failed");
    MyError::api(
        axum::http::StatusCode::BAD_GATEWAY,
        "Payment Provider Unavailable",
        "Could not reach the payment provider. Please try again.",
    )
}

#[cfg(test)]
mod tests {
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

    fn slot(start: &str, end: &str) -> shared::general_models::spot::TimeSlot {
        shared::general_models::spot::TimeSlot {
            start: start.into(),
            end: end.into(),
        }
    }

    #[test]
    fn one_slot_reads_as_a_date_and_a_range() {
        let booked = HashMap::from([("2026-08-14".to_string(), vec![slot("09:00", "11:00")])]);
        assert_eq!(describe(&booked), "Parking · 14 Aug, 09:00–11:00");
    }

    /// The property Stripe's idempotency depends on: same booking, same string, every
    /// time. `Booked` is a HashMap, so without sorting this would vary per call and a
    /// retried checkout would be refused for differing parameters.
    #[test]
    fn the_description_is_deterministic_and_starts_at_the_earliest_slot() {
        let booked = HashMap::from([
            (
                "2026-08-15".to_string(),
                vec![slot("14:00", "15:00"), slot("08:00", "09:00")],
            ),
            ("2026-08-14".to_string(), vec![slot("09:00", "11:00")]),
        ]);

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
        assert_eq!(describe(&HashMap::new()), "Parking");
    }

    fn card(images: usize) -> SpotCard {
        SpotCard {
            title: "Kerkstraat 12".to_string(),
            address: "2000 Antwerpen".to_string(),
            images: (0..images).map(|i| format!("spots/{i}.jpeg")).collect(),
        }
    }

    fn one_slot() -> Booked {
        HashMap::from([("2026-08-14".to_string(), vec![slot("09:00", "11:00")])])
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
