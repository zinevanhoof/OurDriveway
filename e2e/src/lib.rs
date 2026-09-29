//! The whole backend, booted for real and driven over HTTP.
//!
//! What `@SpringBootTest` does in-process has to be done with processes here: every
//! service reads `DATABASE_URL`, `PORT`, … into its own `static CONFIG`, so seven of them
//! cannot share one address space. [`Stack::boot`] instead
//!
//! 1. migrates through `migrator::run_all` — the one thing that migrates anywhere,
//! 2. starts [`fake::Fake`] for LocationIQ and Stripe,
//! 3. spawns the seven binaries `cargo build --workspace --bins` left in `target/debug`,
//!    with an explicit environment and ports 13000–13006,
//! 4. waits for every `/readyz`.
//!
//! Infra is `docker/docker-compose-dev.yml`'s `nats` and `yugabyte`, the same pair the
//! `live_tests` modules use, and assertions only ever look at rows a test created — so
//! all of them can share one cluster with whatever a dev already has in it.
//!
//! **Stop the natively-run dev services first.** The ports do not clash, but the NATS
//! durables are shared by name, so a running dev payment-service takes its share of
//! the e2e stack's events — and acts on them against *real* Stripe, which has never
//! heard of the fake's sessions. The e2e instance never sees the event and the test
//! waiting for it times out.

mod fake;

use std::{
    fs,
    future::Future,
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::Mutex,
    time::{Duration, Instant},
};

pub use fake::{Fake, UNKNOWN_ADDRESS};
use reqwest::{RequestBuilder, Response, StatusCode, header::HeaderMap};
use serde_json::{Value, json};
use uuid::Uuid;

pub const PASSWORD: &str = "E2e-Passw0rd!";
pub const MEDIA_BASE: &str = "https://images.e2e.test";
pub const AWAIT: &str = "x-await-version";
const JWT_SECRET: &str = "e2e-jwt-secret";
const EMAIL_TOKEN_SECRET: &str = "e2e-email-token-secret";
const WEBHOOK_SECRET: &str = "whsec_e2e";
const NATS_URL: &str = "nats://127.0.0.1:4222";
const YSQL: &str = "postgres://yugabyte@127.0.0.1:5433";

/// Binary, port, and the path prefix it answers — the same split as
/// `docker/Caddyfile-dev`, so a test names a path and never a port. notification-service
/// answers only health.
const SERVICES: [(&str, u16, Option<&str>); 7] = [
    ("user-service", 13000, Some("/api/user")),
    ("booking-service", 13001, Some("/api/booking")),
    ("spot-service", 13002, Some("/api/spot")),
    ("view-service", 13003, Some("/api/view")),
    ("media-service", 13004, Some("/api/media")),
    ("notification-service", 13005, None),
    ("payment-service", 13006, Some("/api/payment")),
];

pub struct Stack {
    rt: tokio::runtime::Runtime,
    pub http: reqwest::Client,
    pub fake: Fake,
    logs: PathBuf,
    children: Mutex<Vec<Child>>,
}

/// Kills whatever it holds when dropped, so a boot that panics halfway leaves nothing
/// listening on 130xx for the next run to trip over.
struct Children(Vec<Child>);

impl Drop for Children {
    fn drop(&mut self) {
        kill(&mut self.0);
    }
}

fn kill(children: &mut Vec<Child>) {
    for child in children.iter_mut() {
        let _ = child.kill();
        let _ = child.wait();
    }
    children.clear();
}

impl Stack {
    /// Panics with the failing service's log if anything does not come up.
    pub fn boot() -> Stack {
        // Before the runtime exists: nothing else is running yet, which is what makes
        // the `set_var` sound. `migrator::run_all` reads these five.
        for db in migrator::databases() {
            unsafe { std::env::set_var(db.env, format!("{YSQL}/{}", db.name)) };
        }

        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(migrator::run_all())
            .expect("migrate — is `docker compose -f docker/docker-compose-dev.yml up -d nats yugabyte` running?");
        let fake = rt.block_on(Fake::start());

        let bin = target_dir();
        let logs = bin.join("e2e-logs");
        let _ = fs::remove_dir_all(&logs);
        fs::create_dir_all(&logs).unwrap();

        let mut children = Children(Vec::new());
        for (name, port, _) in SERVICES {
            let exe = bin.join(name);
            assert!(
                exe.exists(),
                "{} is missing — run `cargo build --workspace --bins` first",
                exe.display()
            );
            let log = fs::File::create(logs.join(format!("{name}.log"))).unwrap();
            let child = Command::new(&exe)
                // Nothing inherited, and a cwd with no `apps/…/.env` under it: each main
                // runs `dotenvy::from_filename`, and a dev's own .env must not leak in.
                .env_clear()
                .envs(env(name, port, &fake.base))
                .env("RUST_LOG", std::env::var("RUST_LOG").unwrap_or("info".into()))
                .env("RUST_BACKTRACE", "1")
                .current_dir(&logs)
                .stdout(log.try_clone().unwrap())
                .stderr(log)
                .stdin(Stdio::null())
                .spawn()
                .unwrap_or_else(|e| panic!("spawn {name}: {e}"));
            children.0.push(child);
        }

        let stack = Stack {
            rt,
            http: reqwest::Client::new(),
            fake,
            logs,
            children: Mutex::new(Vec::new()),
        };
        stack.wait_ready(&mut children.0);
        *stack.children.lock().unwrap() = std::mem::take(&mut children.0);
        stack
    }

    /// `/readyz` answers 503 until a service's projectors have replayed, so this is
    /// also the wait for the streams to be caught up.
    fn wait_ready(&self, children: &mut [Child]) {
        let deadline = Instant::now() + Duration::from_secs(120);
        for (i, (name, port, _)) in SERVICES.iter().enumerate() {
            loop {
                if let Ok(Some(status)) = children[i].try_wait() {
                    self.dump_logs();
                    panic!("{name} exited during boot ({status}) — its log is above");
                }
                let ready = self.rt.block_on(async {
                    self.http
                        .get(format!("http://127.0.0.1:{port}/readyz"))
                        .send()
                        .await
                        .is_ok_and(|r| r.status().is_success())
                });
                if ready {
                    break;
                }
                if Instant::now() > deadline {
                    self.dump_logs();
                    panic!("{name} not ready on :{port} after 120s — logs are above");
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    }

    pub fn shutdown(&self) {
        kill(&mut self.children.lock().unwrap());
    }

    /// The tail of every service's log, for a failed run. CI also uploads the files.
    pub fn dump_logs(&self) {
        for (name, _, _) in SERVICES {
            let log = fs::read_to_string(self.logs.join(format!("{name}.log"))).unwrap_or_default();
            let lines: Vec<_> = log.lines().collect();
            eprintln!("\n──── {name} (last 40 lines) ────");
            for line in &lines[lines.len().saturating_sub(40)..] {
                eprintln!("{line}");
            }
        }
        eprintln!("\nfull logs: {}", self.logs.display());
    }

    pub fn block_on<F: Future>(&self, f: F) -> F::Output {
        self.rt.block_on(f)
    }

    // ─── requests ───────────────────────────────────────────────────────────

    pub fn url(&self, path: &str) -> String {
        let (_, port, _) = SERVICES
            .iter()
            .find(|(_, _, prefix)| prefix.is_some_and(|p| path.starts_with(p)))
            .unwrap_or_else(|| panic!("no service answers {path}"));
        format!("http://127.0.0.1:{port}{path}")
    }

    pub fn get(&self, path: &str) -> RequestBuilder {
        self.http.get(self.url(path))
    }

    pub fn post(&self, path: &str) -> RequestBuilder {
        self.http.post(self.url(path))
    }

    pub fn delete(&self, path: &str) -> RequestBuilder {
        self.http.delete(self.url(path))
    }

    pub fn patch(&self, path: &str) -> RequestBuilder {
        self.http.patch(self.url(path))
    }

    // ─── flows every test needs ─────────────────────────────────────────────

    /// Signs up a fresh address. Returns the user id and the `X-Version` to echo.
    pub async fn signup(&self, email: &str) -> (Uuid, String) {
        let res = self
            .post("/api/user/signup")
            .json(&signup_body(email, PASSWORD))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::ACCEPTED, "signup");
        let version = x_version(res.headers());
        (id_of(&version), version)
    }

    /// The link notification-service would have mailed, minted with the same secret.
    pub fn verify_token(&self, user_id: &Uuid) -> String {
        shared::email_token::mint(
            EMAIL_TOKEN_SECRET,
            user_id,
            shared::email_token::Purpose::VerifyEmail,
            3600,
            None,
        )
        .unwrap()
    }

    /// Signed up, verified and logged in.
    pub async fn user(&self) -> User {
        let email = unique_email();
        let (id, _) = self.signup(&email).await;

        let res = self
            .post("/api/user/email/verify")
            .json(&json!({ "token": self.verify_token(&id) }))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::ACCEPTED, "verify");
        let verified = x_version(res.headers());

        let res = self
            .post("/api/user/login")
            .header(AWAIT, &verified)
            .json(&json!({ "email": email, "password": PASSWORD }))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK, "login");
        let token = res.json::<Value>().await.unwrap()["accessToken"]
            .as_str()
            .unwrap()
            .to_string();

        User {
            id,
            token,
            version: verified,
        }
    }

    /// A spot at 500/h, open every day 00:00–23:30. Returns its id and `X-Version`.
    pub async fn spot(&self, host: &User) -> (Uuid, String) {
        self.spot_priced(host, 500).await
    }

    pub async fn spot_priced(&self, host: &User, cents_per_hour: i64) -> (Uuid, String) {
        let mut body = spot_body("1 Grand-Place, 1000 Brussels, Belgium");
        body["pricePerHourCents"] = json!(cents_per_hour);
        let res = self
            .post("/api/spot")
            .bearer_auth(&host.token)
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::ACCEPTED, "create spot");
        let version = x_version(res.headers());
        (id_of(&version), version)
    }

    /// Reserves 10:00–12:00 on `date`. The response is returned as-is so a test can
    /// assert a refusal; [`Self::booking`] is the version that insists.
    pub async fn reserve(&self, renter: &User, spot: &(Uuid, String), date: &str) -> Response {
        self.reserve_slot(renter, spot, date, ("10:00", "12:00")).await
    }

    pub async fn reserve_slot(
        &self,
        renter: &User,
        spot: &(Uuid, String),
        date: &str,
        (start, end): (&str, &str),
    ) -> Response {
        self.post("/api/booking")
            .bearer_auth(&renter.token)
            // booking-service reserves against its mirror of the spot.
            .header(AWAIT, &spot.1)
            .json(&json!({
                "spotId": spot.0,
                "booked": { date: [{ "start": start, "end": end }] },
                "licensePlate": "1-ABC-123",
            }))
            .send()
            .await
            .unwrap()
    }

    pub async fn booking(&self, renter: &User, spot: &(Uuid, String), date: &str) -> (Uuid, String) {
        let res = self.reserve(renter, spot, date).await;
        assert_eq!(res.status(), StatusCode::ACCEPTED, "reserve");
        let version = x_version(res.headers());
        (id_of(&version), version)
    }

    /// Starts checkout for a booking. Returns the session id.
    pub async fn checkout(&self, renter: &User, booking: &(Uuid, String)) -> String {
        let res = self
            .post("/api/payment/session")
            .bearer_auth(&renter.token)
            // payment-service authorizes against its mirror of the booking.
            .header(AWAIT, &booking.1)
            .json(&json!({ "bookingId": booking.0, "returnUrl": "https://e2e.test/return" }))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK, "create session");
        res.json::<Value>().await.unwrap()["sessionId"]
            .as_str()
            .unwrap()
            .to_string()
    }

    /// The card goes through: Stripe marks the session paid and sends the webhook.
    /// Returns the intent id, which is what a refund is issued against.
    pub async fn pay(&self, session_id: &str, booking_id: &Uuid) -> String {
        self.fake.pay(session_id);
        let intent = format!("pi_{}", Uuid::now_v7().simple());
        let res = self.webhook(&succeeded(booking_id, &intent), WEBHOOK_SECRET).await;
        assert_eq!(res.status(), StatusCode::OK, "webhook");
        intent
    }

    /// Posts `body` the way Stripe would, signed with `secret`.
    pub async fn webhook(&self, body: &str, secret: &str) -> Response {
        self.post("/api/payment/webhook")
            .header(
                "stripe-signature",
                stripe_webhook::Webhook::generate_test_header(body, secret, None),
            )
            .body(body.to_string())
            .send()
            .await
            .unwrap()
    }

    /// The renter's own view of a booking, or `None` until view-service has it.
    pub async fn renter_booking(&self, renter: &User, booking_id: &Uuid) -> Option<Value> {
        let res = self
            .get(&format!("/api/view/renter/bookings/{booking_id}"))
            .bearer_auth(&renter.token)
            .send()
            .await
            .unwrap();
        res.status()
            .is_success()
            .then_some(res.json().await.unwrap())
    }

    /// Waits for view-service to show `booking_id` in `status`.
    ///
    /// Polling rather than `X-Await-Version`: the transitions under test are made by a
    /// worker reacting to another service's event, so the client never holds a version
    /// for them — exactly the case the frontend polls for too.
    pub async fn until_status(&self, renter: &User, booking_id: &Uuid, status: &str) {
        eventually(&format!("booking {booking_id} to be {status}"), || async {
            self.renter_booking(renter, booking_id)
                .await
                .filter(|b| b["status"] == status)
        })
        .await;
    }
}

pub struct User {
    pub id: Uuid,
    pub token: String,
    /// `user:<id>@<n>` from the verification — what view-service waits on before a read
    /// that must see this account.
    pub version: String,
}

/// Polls `f` until it answers `Some`, or panics after 20s naming `what`.
///
/// Everything here waits on a broker round trip, so a fixed sleep would be either
/// flaky or slow — same reasoning as `until` in bus/src/projector.rs's live tests.
pub async fn eventually<T, F, Fut>(what: impl std::fmt::Display, mut f: F) -> T
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Option<T>>,
{
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(value) = f().await {
            return value;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

pub fn unique_email() -> String {
    format!("e2e-{}@example.com", Uuid::now_v7().simple())
}

/// `YYYY-MM-DD`, `days` from today — far enough out that the one-hour cancel cutoff and
/// the "not in the past" rule never depend on what time the suite runs.
pub fn date_in(days: u64) -> String {
    (chrono::Utc::now().date_naive() + chrono::Days::new(days)).to_string()
}

pub fn signup_body(email: &str, password: &str) -> Value {
    json!({
        "firstName": "E2e",
        "lastName": "Tester",
        "email": email,
        "password": password,
    })
}

/// `refresh-token=<uuid>` out of a response's `Set-Cookie`, ready to send back.
pub fn refresh_cookie(headers: &HeaderMap) -> String {
    headers
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with("refresh-token="))
        .and_then(|v| v.split(';').next())
        .expect("a refresh-token cookie")
        .to_string()
}

pub fn spot_body(address: &str) -> Value {
    // From midnight so the payout flow can book a slot that has already ended.
    let open = json!([{ "start": "00:00", "end": "23:30" }]);
    json!({
        "title": "E2e test driveway",
        "pricePerHourCents": 500,
        "address": {
            "line1": "1 Grand-Place",
            "line2": null,
            "city": "Brussels",
            "postalCode": "1000",
            "region": null,
            "country": "Belgium",
            "formatted": address,
        },
        "availability": {
            "weekly": {
                "monday": open, "tuesday": open, "wednesday": open, "thursday": open,
                "friday": open, "saturday": open, "sunday": open,
            },
            "single": {},
        },
        "images": [format!("{MEDIA_BASE}/spots/{}.jpeg", Uuid::new_v4().simple())],
    })
}

/// A `payment_intent.succeeded` for `booking_id` — the shape `client/stripe.rs`'s own
/// unit tests verify against.
pub fn succeeded(booking_id: &Uuid, intent: &str) -> String {
    json!({
        "id": format!("evt_{}", Uuid::now_v7().simple()),
        "object": "event",
        // async-stripe rc.8's pinned version; any other logs a mismatch per webhook.
        "api_version": "2026-07-29.dahlia",
        "created": 1_492_774_577,
        "livemode": false,
        "pending_webhooks": 1,
        "type": "payment_intent.succeeded",
        "data": { "object": {
            "id": intent,
            "object": "payment_intent",
            "amount": 1000,
            "currency": "eur",
            "status": "succeeded",
            "metadata": { "booking_id": booking_id.to_string() },
            "capture_method": "automatic",
            "confirmation_method": "automatic",
            "created": 1_492_774_577,
            "livemode": false,
            "payment_method_types": ["card"],
            "amount_capturable": 0,
            "amount_received": 1000,
        }},
    })
    .to_string()
}

pub fn x_version(headers: &HeaderMap) -> String {
    headers
        .get("x-version")
        .expect("an X-Version header")
        .to_str()
        .unwrap()
        .to_string()
}

/// `"spot:019f…@1"` -> the uuid.
pub fn id_of(version: &str) -> Uuid {
    let aggregate = version.split('@').next().unwrap();
    aggregate.split(':').nth(1).unwrap().parse().unwrap()
}

/// `target/debug`, found from this test binary's own path (`target/debug/deps/e2e-…`),
/// so a `CARGO_TARGET_DIR` elsewhere still works.
fn target_dir() -> PathBuf {
    std::env::current_exe()
        .unwrap()
        .parent()
        .and_then(|deps| deps.parent())
        .unwrap()
        .to_path_buf()
}

/// Every variable each service's `Config` reads — see its `.env` for what they mean.
fn env(service: &str, port: u16, fake: &str) -> Vec<(&'static str, String)> {
    let db = |name: &str| ("DATABASE_URL", format!("{YSQL}/{name}"));
    let mut vars = vec![
        ("PORT", port.to_string()),
        ("JWT_SECRET", JWT_SECRET.into()),
        ("NATS_URL", NATS_URL.into()),
    ];
    vars.extend(match service {
        "user-service" => vec![
            db("user"),
            ("MEDIA_BASE", MEDIA_BASE.into()),
            ("EMAIL_TOKEN_SECRET", EMAIL_TOKEN_SECRET.into()),
            ("JWT_EXPIRATION", "15".into()),
            ("REFRESH_TOKEN_EXPIRATION", "15".into()),
            // The suite talks plain HTTP; it reads the cookie from Set-Cookie itself.
            ("COOKIE_SECURE", "false".into()),
        ],
        "booking-service" => vec![db("booking")],
        "spot-service" => vec![
            db("spot"),
            ("MEDIA_BASE", MEDIA_BASE.into()),
            ("LOCATIONIQ_API_KEY", "pk.e2e".into()),
            ("LOCATIONIQ_BASE_URL", fake.into()),
        ],
        // 0 on both sides of the settlement rule, as in the dev .env: a booking that has
        // ended is withdrawable at once, so the payout flow needs no day to pass.
        "view-service" => vec![db("view"), ("SETTLEMENT_SECS", "0".into())],
        "media-service" => vec![
            ("MEDIA_BASE", MEDIA_BASE.into()),
            ("S3_ENDPOINT", "https://s3.e2e.test".into()),
            ("S3_BUCKET", "e2e".into()),
            ("S3_REGION", "auto".into()),
            ("S3_ACCESS_KEY_ID", "e2e".into()),
            ("S3_SECRET_ACCESS_KEY", "e2e".into()),
            ("PRESIGN_EXPIRY_SECS", "300".into()),
            ("MAX_UPLOAD_BYTES", "5000000".into()),
        ],
        "notification-service" => vec![
            ("NOTIFICATIONS_ENABLED", "false".into()),
            ("RESEND_API_KEY", "re_e2e".into()),
            ("MAIL_FROM", "e2e@example.com".into()),
            ("APP_BASE_URL", "https://e2e.test".into()),
            ("COMPANY_NAME", "OurDriveway".into()),
            ("EMAIL_TOKEN_SECRET", EMAIL_TOKEN_SECRET.into()),
            ("VERIFY_TOKEN_TTL_SECS", "3600".into()),
            ("RESET_TOKEN_TTL_SECS", "3600".into()),
            ("TEMPLATE_EMAIL_VERIFICATION", "e2e-verify".into()),
            ("TEMPLATE_PASSWORD_RESET", "e2e-reset".into()),
        ],
        "payment-service" => vec![
            db("payment"),
            ("STRIPE_API_BASE", fake.into()),
            ("STRIPE_SECRET_KEY", "sk_test_e2e".into()),
            ("STRIPE_WEBHOOK_SECRET", WEBHOOK_SECRET.into()),
            ("SETTLEMENT_SECS", "0".into()),
        ],
        other => panic!("no environment for {other}"),
    });
    vars
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_come_out_of_versions() {
        let id = Uuid::now_v7();
        assert_eq!(id_of(&format!("spot:{id}@3")), id);
    }
}
