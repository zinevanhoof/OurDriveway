use async_nats::{HeaderMap, jetstream::Context};
use serde::Serialize;
use shared::{error::myerror::MyError, events::Envelope};

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
    let payload =
        serde_json::to_vec(envelope).map_err(|e| MyError::Bus(format!("serialize event: {e}")))?;

    let mut headers = HeaderMap::new();
    // Within the stream's duplicate_window a retried publish carrying the same
    // event id is discarded by the server rather than appended twice.
    headers.insert("Nats-Msg-Id", envelope.event_id.to_string().as_str());

    let ack = js
        .publish_with_headers(subject, headers, payload.into())
        .await
        .map_err(|e| MyError::Bus(format!("publish: {e}")))?
        .await
        .map_err(|e| MyError::Bus(format!("publish ack: {e}")))?;

    Ok(ack.sequence)
}
