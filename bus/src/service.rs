//! Request/reply between services, for the one thing events are bad at: a value you
//! need *now*, keyed by an id you already hold.
//!
//! Chosen over an HTTP endpoint because it costs nothing that isn't already paid. Every
//! service holds an authenticated NATS connection, so there is no URL to configure per
//! caller per environment, and no second auth surface — every axum route in this codebase
//! sits behind `AuthedJwt`, and a service has no user token to present.
//!
//! Not the NATS Services API. `$SRV` discovery and per-endpoint statistics are a service
//! registry, and across six services whose names are compiled in, that is ceremony.
//!
//! What this deliberately does **not** give you is durability or retries. A reply inbox
//! dies with its caller exactly like an HTTP connection does. That is the correct
//! trade-off only because nothing important travels this way — see the note on
//! [`shared::rpc`].

use std::time::Duration;

use async_nats::Client;
use futures::StreamExt;
use serde::{Serialize, de::DeserializeOwned};
use shared::error::myerror::{MyError, MyResult};

/// How long a caller waits before giving up.
///
/// Short on purpose: every caller of this is decorating something, so a slow answer is
/// worth less than a fast absence. Note that a responder being *down* does not cost this
/// — NATS answers "no responders" immediately — so this only bites when something is
/// listening but wedged.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);

/// Asks `subject` and waits for one reply.
///
/// Every failure — nobody listening, nobody answering in time, a reply that won't decode
/// — arrives as one `Err`, because the caller can do exactly one thing about all of them.
/// Callers are expected to `.ok()` this rather than propagate it.
pub async fn request<Q, R>(nc: &Client, subject: &'static str, query: &Q) -> MyResult<R>
where
    Q: Serialize + Sync,
    R: DeserializeOwned,
{
    let payload = serde_json::to_vec(query)
        .map_err(|e| MyError::Bus(format!("{subject}: encoding request: {e}")))?;

    let message = tokio::time::timeout(
        REQUEST_TIMEOUT,
        nc.request(subject.to_string(), payload.into()),
    )
    .await
    .map_err(|_| MyError::Bus(format!("{subject}: no reply within {REQUEST_TIMEOUT:?}")))?
    // Where "no responders" lands, which is the common case worth reading in a log:
    // the callee is not running, as opposed to running and unhappy.
    .map_err(|e| MyError::Bus(format!("{subject}: {e}")))?;

    serde_json::from_slice(&message.payload)
        .map_err(|e| MyError::Bus(format!("{subject}: decoding reply: {e}")))
}

/// Answers `subject` forever. Spawn it, like [`crate::projector::run`].
///
/// Queue-subscribed, so N replicas share the requests instead of all answering the same
/// one — the same reason a [`crate::Worker`] uses a durable name, arrived at differently.
///
/// A request this cannot decode is still answered, with JSON `null`. Staying silent would
/// be the same outcome for the caller two seconds later, and two seconds is a long time to
/// spend rediscovering that a message was malformed.
pub async fn serve<Q, R, F, Fut>(nc: Client, subject: &'static str, queue: &'static str, handler: F)
where
    Q: DeserializeOwned,
    R: Serialize,
    F: Fn(Q) -> Fut,
    Fut: Future<Output = R>,
{
    let mut requests = match nc
        .queue_subscribe(subject.to_string(), queue.to_string())
        .await
    {
        Ok(sub) => sub,
        Err(e) => {
            tracing::error!(subject, error = %e, "could not subscribe; nothing will answer");
            return;
        }
    };

    tracing::info!(subject, queue, "serving requests");

    while let Some(message) = requests.next().await {
        // A publish rather than a request — fire-and-forget with nowhere to send an
        // answer. Not an error: it is how anyone can watch this subject.
        let Some(reply) = message.reply else { continue };

        let response = match serde_json::from_slice::<Q>(&message.payload) {
            Ok(query) => serde_json::to_vec(&handler(query).await),
            Err(e) => {
                tracing::warn!(subject, error = %e, "undecodable request");
                Ok(b"null".to_vec())
            }
        };

        match response {
            Ok(payload) => {
                if let Err(e) = nc.publish(reply, payload.into()).await {
                    tracing::warn!(subject, error = %e, "could not reply");
                }
            }
            Err(e) => tracing::error!(subject, error = %e, "could not encode reply"),
        }
    }
}
