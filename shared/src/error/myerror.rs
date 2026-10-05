use std::collections::HashMap;

use axum::Json;
use axum::extract::rejection::JsonRejection;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;
use thiserror::Error;

pub type MyResult<T> = Result<T, MyError>;

#[derive(Error, Debug)]
pub enum MyError {
    /// Garde validation failure — rendered as one message per field.
    #[error("Validation failed")]
    Validation(#[from] garde::Report),

    /// Deliberate API error with an HTTP status, a title and one or more detail messages.
    #[error("{title}")]
    Api {
        status: StatusCode,
        title: String,
        detail: Vec<String>,
    },

    // Everything below is an unexpected failure: 500, or 503 when the database could not
    // be reached. The client is told the status and nothing about the cause.
    ///
    /// Carries the diesel error whole rather than a string, which is what lets
    /// `db::is_write_conflict` match on `DatabaseErrorKind` instead of on message text.
    /// The SurrealDB equivalent had no typed surface at all — a write conflict arrived
    /// as an untyped `Internal` whose kind was not stable across access paths, so the
    /// only thing to match was the substring "WriteConflict".
    ///
    /// Narrower than the sqlx variant it replaces: `sqlx::Error` folded connection and
    /// pool failures in with query failures, and diesel splits all three. Hence the two
    /// variants below it.
    #[error(transparent)]
    Database(#[from] diesel::result::Error),
    /// Establishing a connection failed — a bad URL, a refused socket, a wrong
    /// password. Separate from [`Self::Database`] because diesel keeps it separate.
    #[error(transparent)]
    Connection(#[from] diesel::ConnectionError),
    /// The pool could not hand out a connection: exhausted, or timed out waiting.
    ///
    /// `sqlx::Error` had a `PoolTimedOut` arm, so this used to arrive as `Database`.
    /// bb8 reports it out of band, so it needs a home of its own or every
    /// `pool.get().await?` call site would have to map it by hand.
    #[error("connection pool: {0}")]
    Pool(String),
    #[error(transparent)]
    Jwt(#[from] jsonwebtoken::errors::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// Publishing to, or reading from, the event log failed.
    ///
    /// A write handler that hits this has NOT written anything — the publish is
    /// the commit — so returning 500 is honest: nothing happened, retry is safe.
    #[error("event bus: {0}")]
    Bus(String),
}

impl MyError {
    pub fn api(status: StatusCode, title: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::Api {
            status,
            title: title.into(),
            detail: vec![detail.into()],
        }
    }
}

impl From<JsonRejection> for MyError {
    fn from(rejection: JsonRejection) -> Self {
        Self::api(
            rejection.status(),
            "Invalid request body",
            rejection.body_text(),
        )
    }
}

impl IntoResponse for MyError {
    fn into_response(self) -> Response {
        let (status, body) = self.render();
        (status, Json(body)).into_response()
    }
}

/// A server fault as the client sees it: the status, its standard name, and nothing else.
///
/// The body used to carry the error's `Display`, which for a database error is the
/// database's own text — a login during a full disk answered with YugabyteDB's
/// `Write to tablet … rejected. Node … has insufficient disk space`, tablet and node ids
/// included. None of that is the caller's to read or theirs to act on. It goes to the log
/// at the call site instead, which is the only place it was ever useful.
///
/// `detail` is still present, and empty, so the body keeps the one shape every client
/// already parses.
fn opaque(status: StatusCode) -> (StatusCode, serde_json::Value) {
    (
        status,
        json!({
            "status": status.as_u16(),
            "title": status.canonical_reason().unwrap_or("Server Error"),
            "detail": [],
        }),
    )
}

impl MyError {
    /// The status and body this error answers with. Split from `into_response` so the
    /// tests can read the body without an async runtime to collect it.
    fn render(self) -> (StatusCode, serde_json::Value) {
        match self {
            MyError::Validation(report) => {
                // A field can fail several rules, so collect all messages per field.
                let mut errors: HashMap<String, Vec<String>> = HashMap::new();

                for (path, err) in report.iter() {
                    errors
                        .entry(camel(&path.to_string()))
                        .or_default()
                        .push(err.to_string());
                }
                let status = StatusCode::UNPROCESSABLE_ENTITY;
                (
                    status,
                    json!({ "status": status.as_u16(), "title": "Validation failed", "errors": errors }),
                )
            }
            // A 500 someone raised on purpose (`context_internal`, media-service's
            // `internal`). Its words were written for whoever reads the code — "locationiq
            // search parse failed" — and name what sits behind this service, so they are
            // logged and the client gets the same blank 500 as any other fault.
            //
            // Only 500. A 502 or 503 raised by hand is a sentence written *for* the user
            // ("Could not reach the payment provider. Please try again.") with the cause
            // already kept back in the log by whoever raised it.
            MyError::Api {
                status,
                title,
                detail,
            } if status == StatusCode::INTERNAL_SERVER_ERROR => {
                tracing::error!(%title, ?detail, "internal error");
                opaque(status)
            }
            MyError::Api {
                status,
                title,
                detail,
            } => (
                status,
                json!({ "status": status.as_u16(), "title": title, "detail": detail }),
            ),

            // A unique index refused the write. That is not a server fault: it is the
            // same conflict the handler's own pre-read reports as a 409, arriving from
            // the only guard that sees a *concurrent* duplicate.
            //
            // `signup` is the live example. Its `find_by_email` turns the sequential case
            // into a 409, but two requests racing both read `None` before either commits,
            // and `app_user_email_idx` is what actually decides. Reaching here used to
            // mean a double-clicked signup answered **500** — a real conflict wearing the
            // wrong status, and unretryable-looking to a client that should simply be
            // told the address is taken.
            //
            // Deliberately no `detail` from the database: an index name is an internal
            // fact and some carry the value that collided.
            MyError::Database(diesel::result::Error::DatabaseError(
                diesel::result::DatabaseErrorKind::UniqueViolation,
                _,
            )) => {
                let status = StatusCode::CONFLICT;
                (
                    status,
                    json!({ "status": status.as_u16(), "title": "Conflict", "detail": ["That value is already taken."] }),
                )
            }

            // Everything unexpected. 503 when the database could not be reached or the pool
            // had no connection to give: nothing is wrong with the request and it is worth
            // sending again, which is what 503 says and 500 does not. 500 for the rest.
            other => {
                let status = match &other {
                    MyError::Connection(_) | MyError::Pool(_) => StatusCode::SERVICE_UNAVAILABLE,
                    _ => StatusCode::INTERNAL_SERVER_ERROR,
                };
                tracing::error!(error = %other, %status, "request failed");
                opaque(status)
            }
        }
    }
}

/// `license_plates[0]` -> `licensePlates[0]`. A no-op on a name already camelCase.
///
/// Garde builds its paths out of the *Rust* field names, but every request struct on
/// the wire carries `#[serde(rename_all = "camelCase")]` — so a 422's keys were the
/// one snake_case thing left in the whole API, and every client had to undo it. The
/// frontend did, in `lib/serverErrors.ts`, and two components had their own copies of
/// that copy.
///
/// This is exact rather than a heuristic, and the test below is what keeps it exact:
/// no request struct carries a per-field `#[serde(rename = …)]`, so `rename_all` is
/// the only transform standing between a field's Rust name and its wire name, and
/// this reproduces it. Add a per-field rename and the test fails.
///
/// Bracket and dot segments pass through untouched — garde appends `[0]` for a vec
/// element and joins `dive`d fields with `.`, neither of which serde ever sees.
fn camel(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    let mut upper_next = false;

    for c in path.chars() {
        match c {
            '_' => upper_next = true,
            _ if upper_next => {
                out.extend(c.to_uppercase());
                upper_next = false;
            }
            _ => out.push(c),
        }
    }

    out
}

/// Ergonomic `.context_bad_request((title, detail))?` on `Result`, `Option` and
/// `bool`, mirroring the axum-anyhow helpers we replaced.
///
/// Every deliberate API error goes through here rather than through a bare
/// `return Err(MyError::api(…))`. Two reasons beyond taste: the guard and its
/// failure end up on one line, so a reader cannot lose track of which condition
/// produces which status; and the status is named, so nobody has to recognise
/// `StatusCode::UNPROCESSABLE_ENTITY` to know what a branch answers.
pub trait ContextExt<T> {
    fn context_status(self, status: StatusCode, ctx: (&str, &str)) -> MyResult<T>;

    fn context_bad_request(self, ctx: (&str, &str)) -> MyResult<T>
    where
        Self: Sized,
    {
        self.context_status(StatusCode::BAD_REQUEST, ctx)
    }
    fn context_unauthorized(self, ctx: (&str, &str)) -> MyResult<T>
    where
        Self: Sized,
    {
        self.context_status(StatusCode::UNAUTHORIZED, ctx)
    }
    /// Status 403 — distinct from `context_unauthorized`: the caller is
    /// identified, the answer is still no. An unverified login is this, not a 401.
    fn context_forbidden(self, ctx: (&str, &str)) -> MyResult<T>
    where
        Self: Sized,
    {
        self.context_status(StatusCode::FORBIDDEN, ctx)
    }
    fn context_not_found(self, ctx: (&str, &str)) -> MyResult<T>
    where
        Self: Sized,
    {
        self.context_status(StatusCode::NOT_FOUND, ctx)
    }
    /// Status 409 — for a uniqueness check that would otherwise lose to an index
    /// later, somewhere with no request left to answer. Reads as
    /// `find_by_email(…).is_none().context_conflict(…)`: the address must be free.
    fn context_conflict(self, ctx: (&str, &str)) -> MyResult<T>
    where
        Self: Sized,
    {
        self.context_status(StatusCode::CONFLICT, ctx)
    }
    fn context_unprocessable_entity(self, ctx: (&str, &str)) -> MyResult<T>
    where
        Self: Sized,
    {
        self.context_status(StatusCode::UNPROCESSABLE_ENTITY, ctx)
    }
    /// Single-message internal (500) error, replaces anyhow's `.context("...")`.
    ///
    /// `detail` is for the log, not the caller: a 500's body is blank whatever is passed
    /// here (see `render`), so write it for whoever will be reading the log.
    fn context_internal(self, detail: &str) -> MyResult<T>
    where
        Self: Sized,
    {
        self.context_status(
            StatusCode::INTERNAL_SERVER_ERROR,
            ("Internal Server Error", detail),
        )
    }
}

impl<T, E> ContextExt<T> for Result<T, E> {
    fn context_status(self, status: StatusCode, (title, detail): (&str, &str)) -> MyResult<T> {
        self.map_err(|_| MyError::api(status, title, detail))
    }
}

impl<T> ContextExt<T> for Option<T> {
    fn context_status(self, status: StatusCode, (title, detail): (&str, &str)) -> MyResult<T> {
        self.ok_or_else(|| MyError::api(status, title, detail))
    }
}

/// A plain condition, so a guard reads as `must_hold.context_forbidden(…)?`
/// instead of `if !must_hold { return Err(…) }`.
///
/// **`true` passes.** State the condition you require, not the failure you are
/// catching — `user.email_verified.context_forbidden(…)`, and for a uniqueness
/// check `find_by_email(…).is_none().context_conflict(…)`.
impl ContextExt<()> for bool {
    fn context_status(self, status: StatusCode, (title, detail): (&str, &str)) -> MyResult<()> {
        self.then_some(())
            .ok_or_else(|| MyError::api(status, title, detail))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use garde::Validate;

    /// Status, title and detail of what a client receives.
    fn rendered(e: MyError) -> (StatusCode, String, Vec<String>) {
        let (status, body) = e.render();
        let detail = body["detail"]
            .as_array()
            .expect("every error body carries a detail array")
            .iter()
            .map(|d| d.as_str().unwrap().to_string())
            .collect();
        (status, body["title"].as_str().unwrap().to_string(), detail)
    }

    /// The one that started it: a database error's text reached the login screen.
    #[test]
    fn a_database_error_answers_500_and_says_nothing() {
        let leaked = "Write to tablet 34d4511b rejected. Node 5e390399 has insufficient disk space";
        let e = MyError::Database(diesel::result::Error::DatabaseError(
            diesel::result::DatabaseErrorKind::Unknown,
            Box::new(leaked.to_string()),
        ));

        let (status, title, detail) = rendered(e);
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(title, "Internal Server Error");
        assert!(detail.is_empty(), "a server fault must not describe itself: {detail:?}");
    }

    #[test]
    fn every_unexpected_error_answers_without_a_message() {
        let errors = [
            MyError::Database(diesel::result::Error::NotFound),
            MyError::Bus("nats: no responders on BOOKINGS".into()),
            MyError::Io(std::io::Error::other("/var/lib/secret: permission denied")),
        ];
        for e in errors {
            let (status, _, detail) = rendered(e);
            assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
            assert!(detail.is_empty(), "leaked: {detail:?}");
        }
    }

    /// Nothing is wrong with the request, and it is worth sending again.
    #[test]
    fn an_unreachable_database_answers_503_without_a_message() {
        let errors = [
            MyError::Pool("timed out waiting for connection".into()),
            MyError::Connection(diesel::ConnectionError::BadConnection(
                "connection to server at \"db\" (10.42.0.7), port 5433 failed".into(),
            )),
        ];
        for e in errors {
            let (status, title, detail) = rendered(e);
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(title, "Service Unavailable");
            assert!(detail.is_empty(), "leaked: {detail:?}");
        }
    }

    /// `context_internal` words are for the log: they name what is behind the service.
    #[test]
    fn a_deliberate_500_keeps_its_words_out_of_the_body() {
        let e = Err::<(), ()>(())
            .context_internal("locationiq search parse failed")
            .unwrap_err();

        let (status, title, detail) = rendered(e);
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(title, "Internal Server Error");
        assert!(detail.is_empty(), "leaked: {detail:?}");
    }

    /// A 4xx, and a 502 written for the user, are the caller's to read.
    #[test]
    fn errors_meant_for_the_caller_keep_their_message() {
        let (_, _, detail) = rendered(MyError::api(StatusCode::CONFLICT, "Taken", "Slot is taken."));
        assert_eq!(detail, ["Slot is taken."]);

        let (status, _, detail) = rendered(MyError::api(
            StatusCode::BAD_GATEWAY,
            "Payment Provider Unavailable",
            "Could not reach the payment provider. Please try again.",
        ));
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert_eq!(detail.len(), 1);
    }

    /// Still a 409, and still without the index name the database reported.
    #[test]
    fn a_unique_violation_stays_a_conflict() {
        let e = MyError::Database(diesel::result::Error::DatabaseError(
            diesel::result::DatabaseErrorKind::UniqueViolation,
            Box::new("duplicate key value violates unique constraint \"app_user_email_idx\"".to_string()),
        ));

        let (status, _, detail) = rendered(e);
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(detail, ["That value is already taken."]);
    }

    #[test]
    fn context_maps_to_api_error() {
        let err = None::<()>
            .context_bad_request(("Bad Request", "missing"))
            .unwrap_err();
        match err {
            MyError::Api { status, detail, .. } => {
                assert_eq!(status, StatusCode::BAD_REQUEST);
                assert_eq!(detail, vec!["missing".to_string()]);
            }
            other => panic!("expected Api, got {other:?}"),
        }
    }

    /// The polarity is the one thing worth pinning: `true` is the passing case, so
    /// a guard names the condition it requires rather than the failure it catches.
    /// Inverted, every check in the codebase would silently mean its opposite.
    #[test]
    fn a_bool_guard_passes_when_true() {
        assert!(true.context_forbidden(("Nope", "denied")).is_ok());

        let err = false
            .context_conflict(("Taken", "already exists"))
            .unwrap_err();
        match err {
            MyError::Api { status, title, .. } => {
                assert_eq!(status, StatusCode::CONFLICT);
                assert_eq!(title, "Taken");
            }
            other => panic!("expected Api, got {other:?}"),
        }
    }

    #[test]
    fn garde_reports_every_failure_including_multiple_per_field() {
        #[derive(Validate)]
        struct Req {
            #[garde(email)]
            email: String,
            // two rules on one field -> two report entries with the same path
            #[garde(length(min = 5), alphanumeric)]
            name: String,
        }
        let report = Req {
            email: "nope".into(),
            name: "a!".into(),
        }
        .validate()
        .unwrap_err();
        let name_errors = report
            .iter()
            .filter(|(p, _)| p.to_string() == "name")
            .count();
        assert_eq!(
            name_errors, 2,
            "grouping must keep both messages, not overwrite"
        );
    }

    /// `camel` has to agree with `#[serde(rename_all = "camelCase")]`, because that
    /// is what named the field in the request the client sent.
    #[test]
    fn garde_paths_are_camel_cased_like_serde_renames_them() {
        assert_eq!(camel("first_name"), "firstName");
        assert_eq!(camel("price_per_hour_cents"), "pricePerHourCents");

        // Already camel, or a single word: untouched.
        assert_eq!(camel("email"), "email");
        assert_eq!(camel("licensePlates"), "licensePlates");

        // Garde's own syntax rides along unharmed — `[i]` for a vec element, `.` for
        // a `dive`d field. Serde never sees either.
        assert_eq!(camel("license_plates[0]"), "licensePlates[0]");
        assert_eq!(camel("availability.single"), "availability.single");
        assert_eq!(
            camel("availability.price_per_hour"),
            "availability.pricePerHour"
        );

        // A digit after the underscore: `to_uppercase` is a no-op on it and the
        // underscore still goes, which is exactly what serde does with `line_1`.
        assert_eq!(camel("line_1"), "line1");
    }

    /// The claim `camel` rests on: `rename_all` is the ONLY thing between a Rust
    /// field name and its wire name, so reproducing `rename_all` is exact.
    ///
    /// A per-field `#[serde(rename = "...")]` anywhere in `requests/` breaks that,
    /// silently — the 422 would name a field the client does not have. This fails
    /// loudly instead.
    #[test]
    fn no_request_struct_carries_a_per_field_serde_rename() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src/requests");
        let mut offenders = Vec::new();

        fn walk(dir: &std::path::Path, offenders: &mut Vec<String>) {
            for entry in std::fs::read_dir(dir).expect("requests/ must exist") {
                let path = entry.expect("readable entry").path();
                if path.is_dir() {
                    walk(&path, offenders);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let src = std::fs::read_to_string(&path).expect("readable file");
                    for (i, line) in src.lines().enumerate() {
                        // `rename_all` is the container-level one, and the point of
                        // this test. Only a bare `rename =` is the problem.
                        if line.contains("serde(rename") && !line.contains("rename_all") {
                            offenders.push(format!("{}:{}", path.display(), i + 1));
                        }
                    }
                }
            }
        }

        walk(std::path::Path::new(dir), &mut offenders);

        assert!(
            offenders.is_empty(),
            "a per-field serde rename makes `camel` wrong for that field — teach \
             `camel` about it, or drop the rename:\n{}",
            offenders.join("\n")
        );
    }
}
