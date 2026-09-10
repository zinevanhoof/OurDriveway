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

    // Everything below is an unexpected failure -> 500.
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
        let (status, body) = match self {
            MyError::Validation(report) => {
                // A field can fail several rules, so collect all messages per field.
                let mut errors: HashMap<String, Vec<String>> = HashMap::new();

                for (path, err) in report.iter() {
                    errors
                        .entry(path.to_string())
                        .or_default()
                        .push(err.to_string());
                }
                let status = StatusCode::UNPROCESSABLE_ENTITY;
                (
                    status,
                    json!({ "status": status.as_u16(), "title": "Validation failed", "errors": errors }),
                )
            }
            MyError::Api {
                status,
                title,
                detail,
            } => (
                status,
                json!({ "status": status.as_u16(), "title": title, "detail": detail }),
            ),
            other => {
                let status = StatusCode::INTERNAL_SERVER_ERROR;
                (
                    status,
                    json!({ "status": status.as_u16(), "title": "Internal Server Error", "detail": [other.to_string()] }),
                )
            }
        };
        (status, Json(body)).into_response()
    }
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
}
