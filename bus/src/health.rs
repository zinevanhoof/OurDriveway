use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use async_nats::connection::State;
use axum::{Router, http::StatusCode, routing::get};

/// Tracks whether this instance is fit to serve traffic.
///
/// The distinction that matters: `/healthz` means "the process is alive",
/// `/readyz` means "my projections are current". An instance still replaying the
/// log answers reads from a half-built database, so Caddy's `health_uri` must
/// point at `/readyz` — that check is what keeps a booting replica out of
/// rotation, not a comment somewhere.
pub struct Readiness {
    client: async_nats::Client,
    streams: HashMap<&'static str, StreamState>,
}

struct StreamState {
    caught_up: AtomicBool,
    /// Set once a lane dies, and never cleared. See [`Readiness::mark_failed`].
    failed: AtomicBool,
}

impl Readiness {
    pub fn new(client: async_nats::Client, streams: &[&'static str]) -> Arc<Self> {
        Arc::new(Self {
            client,
            streams: streams
                .iter()
                .map(|name| {
                    (
                        *name,
                        StreamState {
                            caught_up: AtomicBool::new(false),
                            failed: AtomicBool::new(false),
                        },
                    )
                })
                .collect(),
        })
    }

    /// Called by a projector once its stream's consumer is drained.
    /// Idempotent, and quiet when it is. Every replica calls this on every poll of
    /// the shared cursor — four times a second — so logging unconditionally buried the
    /// log in a line that says nothing after the first one.
    pub fn mark_caught_up(&self, stream: &str) {
        if let Some(s) = self.streams.get(stream)
            && !s.caught_up.swap(true, Ordering::AcqRel)
        {
            tracing::info!(stream, "caught up");
        }
    }

    /// Called when a projector hits an error it can't apply.
    ///
    /// It must stop rather than skip the message: skipping makes this projection
    /// permanently disagree with the log, which is far worse than being drained.
    /// Flipping readiness is how the instance gets taken out of rotation.
    ///
    /// **Sticky, and that is the point.** The readiness poller keeps running after the
    /// projector dies; once another replica picks up its unacked messages, the consumer
    /// reports drained and [`Self::mark_caught_up`] would put this instance straight
    /// back into rotation with a projector that is never coming back. Clearing it needs
    /// a restart, which is what "a diagnosable stopped replica" means.
    pub fn mark_failed(&self, stream: &str) {
        if let Some(s) = self.streams.get(stream) {
            s.caught_up.store(false, Ordering::Release);
            if !s.failed.swap(true, Ordering::AcqRel) {
                tracing::error!(stream, "projector stalled — instance not ready");
            }
        }
    }

    /// `Ok(())` when serving traffic is safe, `Err(reason)` otherwise.
    fn status(&self) -> Result<(), String> {
        // Checked live rather than cached: a projector that is caught up but whose
        // connection has dropped is not receiving new events, so it is quietly
        // going stale even though its last action succeeded.
        match self.client.connection_state() {
            State::Connected => {}
            other => return Err(format!("nats {other:?}")),
        }

        // Checked before `caught_up`, and separately, so the reason is the useful
        // one: a stalled lane reads very differently from a cold start.
        let failed: Vec<_> = self
            .streams
            .iter()
            .filter(|(_, s)| s.failed.load(Ordering::Acquire))
            .map(|(name, _)| *name)
            .collect();
        if !failed.is_empty() {
            return Err(format!("projector stalled: {}", failed.join(", ")));
        }

        let lagging: Vec<_> = self
            .streams
            .iter()
            .filter(|(_, s)| !s.caught_up.load(Ordering::Acquire))
            .map(|(name, _)| *name)
            .collect();

        if lagging.is_empty() {
            Ok(())
        } else {
            Err(format!("replaying: {}", lagging.join(", ")))
        }
    }
}

/// `/healthz` and `/readyz`, mergeable into any service's router.
pub fn routes<S>(readiness: Arc<Readiness>) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        // Liveness: deliberately does not consult NATS. If this returned 503 while
        // the broker was down, an orchestrator would restart a perfectly healthy
        // process and achieve nothing.
        .route("/healthz", get(|| async { "ok" }))
        .route(
            "/readyz",
            get(async move || {
                let ready = readiness.clone();
                match ready.status() {
                    Ok(()) => (StatusCode::OK, "ready".to_string()),
                    Err(reason) => (StatusCode::SERVICE_UNAVAILABLE, reason),
                }
            }),
        )
}

// `await_applied`, `APPLIED_TIMEOUT` and the `applied` watch that fed them are gone.
//
// They published `ack_floor.stream_sequence` from a stream's one consumer, so a
// worker could ask "has my projection consumed the event that woke me". That number
// stopped meaning anything useful once applies ran concurrently: the floor sits behind
// every in-flight message, so a refund would stall on events the worker does not care
// about — and while the streams were partitioned it had no single value at all.
//
// The question is better asked per aggregate anyway, which is what the event already
// carries — `bus::await_version::reached(db, table, id, envelope.version, timeout)`.
// Its one caller was payment-service's `BookingWorker`.
