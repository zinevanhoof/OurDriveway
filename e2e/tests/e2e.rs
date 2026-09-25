//! Whole-backend flows. Every test is `--ignored`, like the `live_tests` modules, so a
//! plain `cargo test --workspace` stays offline:
//!
//! ```sh
//! docker compose -f docker/docker-compose-dev.yml up -d nats yugabyte
//! cargo build --workspace --bins
//! cargo test --workspace -- --ignored            # this plus every live test
//! cargo test --test e2e -- --ignored booking     # a filter, as usual
//! ```
//!
//! On a failure the tail of every service's log is printed, and the full files are in
//! `target/debug/e2e-logs/`.

use std::sync::OnceLock;

use e2e::{
    AWAIT, PASSWORD, Stack, UNKNOWN_ADDRESS, date_in, eventually, refresh_cookie, signup_body,
    spot_body, succeeded, unique_email, x_version,
};
use libtest_mimic::{Arguments, Failed, Trial};
use reqwest::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

static STACK: OnceLock<Stack> = OnceLock::new();

/// Registers each `async fn(&Stack)` as an ignored trial under its own name.
macro_rules! trials {
    ($($test:ident),* $(,)?) => {
        vec![$(
            Trial::test(stringify!($test), || run(|s| Box::pin($test(s)))).with_ignored_flag(true)
        ),*]
    };
}

fn run(
    test: impl FnOnce(&'static Stack) -> std::pin::Pin<Box<dyn Future<Output = ()>>>,
) -> Result<(), Failed> {
    let stack = STACK.get().expect("the stack is booted before any trial runs");
    stack.block_on(test(stack));
    Ok(())
}

fn main() {
    let args = Arguments::from_args();
    let trials = trials![
        auth_session_lifecycle,
        spot_reaches_the_read_model,
        payment_confirms_a_booking,
        racing_reserves_book_once,
        release_voids_checkout_and_cancel_refunds,
        host_onboards_and_withdraws,
        refusals,
    ];

    // Booting is ~a minute of work, so only when a trial will actually run — a
    // workspace-wide `-- --ignored some_bus_test` must not pay for it.
    let selected = trials.iter().any(|t| {
        let name = t.name();
        let matches = args.filter.as_deref().is_none_or(|f| {
            if args.exact { name == f } else { name.contains(f) }
        });
        matches && !args.skip.iter().any(|s| name.contains(s.as_str()))
    });
    if (args.ignored || args.include_ignored) && !args.list && selected {
        let _ = STACK.set(Stack::boot());
    }

    let conclusion = libtest_mimic::run(&args, trials);

    if let Some(stack) = STACK.get() {
        if conclusion.has_failed() {
            stack.dump_logs();
        }
        stack.shutdown();
    }
    conclusion.exit();
}

// ─── the flows ──────────────────────────────────────────────────────────────

/// Signup through logout, and that the account reaches view-service.
async fn auth_session_lifecycle(s: &Stack) {
    let email = unique_email();
    let (id, signed_up) = s.signup(&email).await;

    let res = s
        .post("/api/user/signup")
        .header(AWAIT, &signed_up)
        .json(&signup_body(&email, PASSWORD))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CONFLICT, "same address twice");

    let email = &email;
    let login = |password: &'static str, version: String| async move {
        s.post("/api/user/login")
            .header(AWAIT, version)
            .json(&json!({ "email": email, "password": password }))
            .send()
            .await
            .unwrap()
    };

    let res = login("Wr0ng-password!", signed_up.clone()).await;
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED, "wrong password");
    let res = login(PASSWORD, signed_up).await;
    assert_eq!(res.status(), StatusCode::FORBIDDEN, "unverified");

    let res = s
        .post("/api/user/email/verify")
        .json(&json!({ "token": s.verify_token(&id) }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::ACCEPTED, "verify");
    let verified = x_version(res.headers());

    let res = login(PASSWORD, verified.clone()).await;
    assert_eq!(res.status(), StatusCode::OK, "login");
    let cookie = refresh_cookie(res.headers());
    let token = res.json::<Value>().await.unwrap()["accessToken"]
        .as_str()
        .unwrap()
        .to_string();

    let res = s
        .get("/api/view/account")
        .bearer_auth(&token)
        .header(AWAIT, &verified)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK, "account");
    assert_eq!(res.json::<Value>().await.unwrap()["id"], json!(id));

    let refresh = |cookie: String| async move {
        s.post("/api/user/session/refresh")
            .header("cookie", cookie)
            .send()
            .await
            .unwrap()
    };

    let res = refresh(cookie).await;
    assert_eq!(res.status(), StatusCode::OK, "refresh");
    let rotated = refresh_cookie(res.headers());

    let res = s
        .post("/api/user/session/logout")
        .header("cookie", &rotated)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK, "logout");

    let res = refresh(rotated).await;
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED, "refresh after logout");
}

/// SPOTS → view-service's projector, read back from both sides, and an address the
/// geocoder cannot place never becomes a spot.
async fn spot_reaches_the_read_model(s: &Stack) {
    let host = s.user().await;
    let (spot_id, version) = s.spot(&host).await;

    for path in [
        format!("/api/view/public/spots/{spot_id}"),
        format!("/api/view/host/spots/{spot_id}"),
    ] {
        let res = s
            .get(&path)
            .bearer_auth(&host.token)
            .header(AWAIT, &version)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK, "{path}");
        let spot: Value = res.json().await.unwrap();
        assert_eq!(spot["id"], json!(spot_id), "{path}");
        assert_eq!(spot["title"], "E2e test driveway", "{path}");
    }

    let res = s
        .post("/api/spot")
        .bearer_auth(&host.token)
        .json(&spot_body(UNKNOWN_ADDRESS))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST, "ungeocodable address");
}

/// Reserve → checkout → Stripe says paid → the booking is confirmed, across four
/// services and three streams.
async fn payment_confirms_a_booking(s: &Stack) {
    let host = s.user().await;
    let renter = s.user().await;
    let spot = s.spot(&host).await;
    let booking = s.booking(&renter, &spot, &date_in(7)).await;

    let res = s
        .get(&format!("/api/view/renter/bookings/{}", booking.0))
        .bearer_auth(&renter.token)
        .header(AWAIT, &booking.1)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let view: Value = res.json().await.unwrap();
    assert_eq!(view["status"], "reserved");
    // Two hours at 500/h, priced by booking-service — no client sent a figure.
    assert_eq!(view["amount"], 1000);

    let session = s.checkout(&renter, &booking).await;
    assert_eq!(
        s.checkout(&renter, &booking).await,
        session,
        "resuming checkout hands back the same session, never a second payable one"
    );

    let renter = &renter;
    let state = |session: String| async move {
        let res = s
            .get(&format!("/api/payment/session/{session}"))
            .bearer_auth(&renter.token)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK, "session state");
        res.json::<Value>().await.unwrap()
    };
    assert_eq!(state(session.clone()).await["paid"], false);

    s.pay(&session, &booking.0).await;
    assert_eq!(state(session).await["paid"], true);
    s.until_status(&renter, &booking.0, "confirmed").await;

    let res = s.reserve(&host, &spot, &date_in(8)).await;
    assert_eq!(res.status(), StatusCode::CONFLICT, "a host booking their own spot");
}

/// The read-committed guarantee docker-compose-dev.yml measured with two ysqlsh
/// sessions, end to end: two renters, one slot, exactly one booking.
async fn racing_reserves_book_once(s: &Stack) {
    let host = s.user().await;
    let (a, b) = tokio::join!(s.user(), s.user());
    let spot = s.spot(&host).await;
    let date = date_in(7);

    let (ra, rb) = tokio::join!(s.reserve(&a, &spot, &date), s.reserve(&b, &spot, &date));
    let mut statuses = [ra.status(), rb.status()];
    statuses.sort();
    assert_eq!(statuses, [StatusCode::ACCEPTED, StatusCode::CONFLICT]);
}

/// The two ways money is unwound: an unpaid hold voids its checkout session, a paid
/// booking cancelled in time is refunded.
async fn release_voids_checkout_and_cancel_refunds(s: &Stack) {
    let host = s.user().await;
    let renter = s.user().await;
    let spot = s.spot(&host).await;

    let unpaid = s.booking(&renter, &spot, &date_in(7)).await;
    let session = s.checkout(&renter, &unpaid).await;
    let res = s
        .delete(&format!("/api/booking/{}", unpaid.0))
        .bearer_auth(&renter.token)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::ACCEPTED, "release");
    eventually("the session to be expired at Stripe", || async {
        s.fake.expired(&session).then_some(())
    })
    .await;
    s.until_status(&renter, &unpaid.0, "released").await;

    let paid = s.booking(&renter, &spot, &date_in(8)).await;
    let session = s.checkout(&renter, &paid).await;
    let intent = s.pay(&session, &paid.0).await;
    s.until_status(&renter, &paid.0, "confirmed").await;

    let res = s
        .post(&format!("/api/booking/{}/cancel", paid.0))
        .bearer_auth(&renter.token)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::ACCEPTED, "cancel");
    eventually("the payment to be refunded at Stripe", || async {
        s.fake.refunded(&intent).then_some(())
    })
    .await;
    s.until_status(&renter, &paid.0, "cancelled").await;
}

/// Connect onboarding through a withdrawal. Money leaves only for an onboarded
/// account and only once earned, a transfer Stripe refuses puts it back, and a paid
/// payout cannot be taken twice.
async fn host_onboards_and_withdraws(s: &Stack) {
    let host = s.user().await;
    let renter = s.user().await;

    let connect = |version: String| {
        let token = host.token.clone();
        async move {
            let res = s
                .get("/api/payment/connect/account")
                .bearer_auth(token)
                .header(AWAIT, version)
                .send()
                .await
                .unwrap();
            assert_eq!(res.status(), StatusCode::OK, "connect status");
            res.json::<Value>().await.unwrap()
        }
    };
    let onboarding = |version: String| {
        s.post("/api/payment/connect/session")
            .bearer_auth(&host.token)
            .header(AWAIT, version)
            .send()
    };

    // Accounts v2 fixes the country at creation, so nothing is created without one.
    assert_eq!(connect(host.version.clone()).await["state"], "needs_country");
    let res = onboarding(host.version.clone()).await.unwrap();
    assert_eq!(res.status(), StatusCode::CONFLICT, "onboarding without a country");

    let res = s
        .patch("/api/user")
        .bearer_auth(&host.token)
        .json(&json!({ "country": "BE" }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::ACCEPTED, "set country");
    let with_country = x_version(res.headers());
    assert_eq!(connect(with_country.clone()).await["state"], "none");

    let res = onboarding(with_country.clone()).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK, "account session");
    let secret: Value = res.json().await.unwrap();
    assert!(secret["clientSecret"].as_str().is_some_and(|s| !s.is_empty()));
    assert_eq!(connect(with_country.clone()).await["state"], "onboarding");

    s.fake.onboard(&host.id);
    let status = connect(with_country).await;
    assert_eq!(status["state"], "enabled");
    assert_eq!(status["bankLast4"], "4321");

    let withdraw = |cents: i64| {
        s.post("/api/payment/payout")
            .bearer_auth(&host.token)
            .json(&json!({ "amountCents": cents }))
            .send()
    };
    let res = withdraw(1_000).await.unwrap();
    assert_eq!(res.status(), StatusCode::CONFLICT, "nothing earned yet");

    // Today 00:00–00:30 in Brussels is always behind UTC's today, so this booking has
    // ended before it is made — and with SETTLEMENT_SECS=0, settled the moment it is paid.
    let spot = s.spot_priced(&host, 10_000).await;
    let res = s
        .reserve_slot(&renter, &spot, &date_in(0), ("00:00", "00:30"))
        .await;
    assert_eq!(res.status(), StatusCode::ACCEPTED, "reserve an ended slot");
    let version = x_version(res.headers());
    let booking = (e2e::id_of(&version), version);
    let session = s.checkout(&renter, &booking).await;
    s.pay(&session, &booking.0).await;
    s.until_status(&renter, &booking.0, "confirmed").await;

    // Polled rather than awaited by version: every change here is made by a worker, so
    // there is no version in the client's hands — and `X-Await-Version` gives up after
    // two seconds and serves what it has, which an equality check would mistake for truth.
    let balance_is = |cents: i64| {
        let token = host.token.clone();
        eventually(format!("a withdrawable balance of {cents}"), move || {
            let token = token.clone();
            async move {
                let res = s
                    .get("/api/view/host/balance")
                    .bearer_auth(token)
                    .send()
                    .await
                    .unwrap();
                let balance: Value = res.json().await.unwrap();
                (balance["availableCents"] == cents).then_some(())
            }
        })
    };
    balance_is(5_000).await;

    // payment-service learns of the confirmation from its own BOOKINGS mirror, which
    // can trail view-service's — so retry until the money is there rather than guess.
    s.fake.refuse_transfers(&host.id, true);
    eventually("the earnings to be withdrawable", || async {
        (withdraw(5_000).await.unwrap().status() == StatusCode::ACCEPTED).then_some(())
    })
    .await;
    eventually("Stripe to refuse the transfer", || async {
        (s.fake.refusals_to(&host.id) == 1).then_some(())
    })
    .await;

    // Refused for asking more than there is, rather than because there is nothing —
    // which is only true once the refused payout stops counting against the balance.
    // Both are 409s; the title says which.
    let refusal = |cents: i64| async move {
        let res = withdraw(cents).await.unwrap();
        assert_eq!(res.status(), StatusCode::CONFLICT, "withdraw {cents}");
        res.json::<Value>().await.unwrap()["title"].clone()
    };
    eventually("a refused payout to be put back", || async {
        (refusal(5_001).await == "More Than Available").then_some(())
    })
    .await;

    // The minimum is the request validator's, so it answers before the balance is read.
    let res = withdraw(999).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY, "below the minimum");

    s.fake.refuse_transfers(&host.id, false);
    let res = withdraw(5_000).await.unwrap();
    assert_eq!(res.status(), StatusCode::ACCEPTED, "withdraw the whole balance again");
    eventually("the transfer", || async {
        (s.fake.transfers_to(&host.id) == [5_000]).then_some(())
    })
    .await;
    balance_is(0).await;
    assert_eq!(refusal(1_000).await, "Nothing to Withdraw", "the same money twice");
}

/// What a caller without the right credentials or with a malformed body gets.
async fn refusals(s: &Stack) {
    let user = s.user().await;

    // No `Authorization` at all is a 400 — `AuthedJwt` rejects the missing header as a
    // malformed request — and a token that does not verify is a 401.
    for path in [
        "/api/spot",
        "/api/booking",
        "/api/payment/session",
        "/api/media/upload-url",
    ] {
        let res = s.post(path).json(&json!({})).send().await.unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST, "{path} without a token");
    }
    let res = s
        .get("/api/view/account")
        .bearer_auth("not-a-jwt")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED, "a forged token");

    let res = s
        .post("/api/user/signup")
        .json(&signup_body(&unique_email(), "short"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY, "weak password");

    let res = s
        .post("/api/booking")
        .bearer_auth(&user.token)
        .json(&json!({
            "spotId": Uuid::now_v7(),
            "booked": { "2000-01-01": [{ "start": "10:00", "end": "12:00" }] },
            "licensePlate": "1-ABC-123",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY, "a date in the past");

    let res = s
        .webhook(&succeeded(&Uuid::now_v7(), "pi_forged"), "whsec_not_ours")
        .await;
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED, "a webhook Stripe did not sign");

    // The only check between one renter and confirming another's booking.
    let host = s.user().await;
    let spot = s.spot(&host).await;
    let booking = s.booking(&user, &spot, &date_in(7)).await;
    let res = s
        .post("/api/payment/session")
        .bearer_auth(&host.token)
        .header(AWAIT, &booking.1)
        .json(&json!({ "bookingId": booking.0, "returnUrl": "https://e2e.test/return" }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN, "paying for someone else's booking");
}
