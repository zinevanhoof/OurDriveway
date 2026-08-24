//! The reply every write in the system shares.

use axum::{Json, http::StatusCode};
use serde::Serialize;

/// What a write answers with: the position its event landed at in the log.
///
/// One definition rather than one per service — it used to be copied into user-,
/// spot- and booking-service's route modules, each with its own `format!` of the
/// same string.
#[derive(Serialize)]
pub struct AcceptedResponse {
    /// `"SPOTS:4712"`. Built by `bus::format_seq`, and read back by the
    /// `bus::await_seq` layer when the client echoes it as `X-Await-Seq`.
    pub seq: String,
}

/// The standard answer to a write: 202 and where it landed.
///
/// 202, not 200/201: the event is committed to the log, but the projections that
/// answer reads — the publishing service's *and* view-service's — are still
/// catching up. `seq` is how the caller waits for its own write on the next read.
///
/// Takes the string already formatted rather than a stream and a sequence,
/// because the formatter lives in `bus` and `bus` depends on this crate — not the
/// other way round. Call sites read `accepted(format_seq(STREAM_SPOTS, seq))`,
/// which is also the honest description: this crate owns the response shape, `bus`
/// owns the log position inside it.
pub fn accepted(seq: String) -> (StatusCode, Json<AcceptedResponse>) {
    (StatusCode::ACCEPTED, Json(AcceptedResponse { seq }))
}
