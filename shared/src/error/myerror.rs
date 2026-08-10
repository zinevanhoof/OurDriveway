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
    #[error(transparent)]
    Database(#[from] surrealdb::Error),
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

    pub fn unauthorized(title: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::api(StatusCode::UNAUTHORIZED, title, detail)
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

/// Ergonomic `.context_bad_request((title, detail))?` on `Result` and `Option`,
/// mirroring the axum-anyhow helpers we replaced.
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
    fn context_not_found(self, ctx: (&str, &str)) -> MyResult<T>
    where
        Self: Sized,
    {
        self.context_status(StatusCode::NOT_FOUND, ctx)
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
