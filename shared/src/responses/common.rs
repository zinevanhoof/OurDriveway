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
    /// `bus::await_version` layer when the client echoes it as `X-Await-Version`.
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

/// What `POST /internal/backfill` answers: how many events it enqueued.
#[derive(Serialize)]
pub struct BackfilledResponse {
    pub events: usize,
}

/// 200, not the 202 every other write here answers with.
///
/// The difference is real rather than cosmetic: 202 means "accepted, and the
/// projections have not caught up", which is why it comes with a token to wait on.
/// A backfill has no such token — it re-emits many aggregates at once, and there is
/// no single version to wait for. What it *can* say truthfully is that the work it
/// was asked to do is finished: the events are in `_outbox` and the relay carries
/// them from there.
pub fn backfilled(events: usize) -> (StatusCode, Json<BackfilledResponse>) {
    (StatusCode::OK, Json(BackfilledResponse { events }))
}
