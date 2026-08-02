use async_nats::jetstream::context::PublishErrorKind;
use async_nats::jetstream::stream::LastRawMessageErrorKind;
use async_nats::jetstream::{Context, message::PublishMessage};
use serde::Serialize;
use shared::{error::myerror::MyError, events::Envelope};

/// Publishing lost a race: the subject moved on since the sequence we asserted.
///
/// Distinct from `Failed` because it is not an error in any operational sense —
/// somebody else simply wrote first, and the caller is expected to catch up and
/// retry rather than surface a 500.
#[derive(Debug)]
pub enum PublishError {
    Stale,
    Failed(MyError),
}

impl From<PublishError> for MyError {
    fn from(e: PublishError) -> Self {
        match e {
            // Only reachable if a caller `?`s a CAS publish instead of running the
            // retry itself. 409 is the honest status: the write was refused because
            // the world changed underneath it, and retrying may well succeed.
            PublishError::Stale => MyError::api(
                axum::http::StatusCode::CONFLICT,
                "Conflict",
                "Someone else changed this first. Please try again.",
            ),
            PublishError::Failed(e) => e,
        }
    }
}

/// Appends an event and returns the stream sequence it landed at.
///
/// The append **is** the commit. A handler publishes and returns; it never writes
/// to its own database, because the projector will apply this same event a moment
/// later — writing in both places is a dual write with no atomicity between them,
/// and the two copies drift the first time one of them fails.
pub async fn publish<T: Serialize>(
    js: &Context,
    subject: String,
    envelope: &Envelope<T>,
) -> Result<u64, MyError> {
    publish_expecting(js, subject, envelope, None)
        .await
        .map_err(Into::into)
}

/// `publish`, optionally conditional on the subject's current head.
///
/// With `expected = Some(n)` the server appends only if the last message on this
/// subject sits at sequence `n`, and it evaluates that atomically with the append.
/// This is the only serialization point in the system: two instances can both read
/// a synced projection and both decide to write, and exactly one of them wins.
///
/// It fails **closed** — a projection lagging behind the log asserts a sequence
/// that is already stale and is refused, so staleness costs a retry rather than a
/// lost update. `Some(0)` asserts the subject is still empty, which is what makes
/// the very first write to a subject safe as well.
///
/// Nothing here is booking-specific. Any per-entity invariant that needs
/// serializing can shard its events onto one subject and use this.
pub async fn publish_expecting<T: Serialize>(
    js: &Context,
    subject: String,
    envelope: &Envelope<T>,
    expected: Option<u64>,
) -> Result<u64, PublishError> {
    let payload = serde_json::to_vec(envelope)
        .map_err(|e| PublishError::Failed(MyError::Bus(format!("serialize event: {e}"))))?;

    // Within the stream's duplicate_window a retried publish carrying the same
    // event id is discarded by the server rather than appended twice.
    let mut message = PublishMessage::build()
        .message_id(envelope.event_id.to_string())
        .payload(payload.into());

    if let Some(seq) = expected {
        message = message.expected_last_subject_sequence(seq);
    }

    let ack = js
        .send_publish(subject, message)
        .await
        .map_err(|e| PublishError::Failed(MyError::Bus(format!("publish: {e}"))))?
        .await;

    match ack {
        Ok(ack) => Ok(ack.sequence),
        // Typed, not a string match against the server's -ERR text: async-nats
        // already maps JetStream error 10071 onto this kind.
        Err(e) if e.kind() == PublishErrorKind::WrongLastSequence => Err(PublishError::Stale),
        Err(e) => Err(PublishError::Failed(MyError::Bus(format!(
            "publish ack: {e}"
        )))),
    }
}

/// Sequence of the last message on `subject`, or 0 if it has never been written.
///
/// This is the value to assert in the next `publish_expecting`, and the position a
/// caller waits for its projector to reach after losing a CAS race.
pub async fn subject_head(js: &Context, stream: &str, subject: &str) -> Result<u64, MyError> {
    let handle = js
        .get_stream(stream)
        .await
        .map_err(|e| MyError::Bus(format!("get stream {stream}: {e}")))?;

    match handle.get_last_raw_message_by_subject(subject).await {
        Ok(message) => Ok(message.sequence),
        // An untouched subject is the normal case for a spot's first booking, not a
        // failure — and 0 is exactly what `expected_last_subject_sequence` wants in
        // order to assert "this subject is still empty".
        Err(e) if e.kind() == LastRawMessageErrorKind::NoMessageFound => Ok(0),
        Err(e) => Err(MyError::Bus(format!("subject head {subject}: {e}"))),
    }
}
