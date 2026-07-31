use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use async_nats::connection::State;
use axum::{Router, http::StatusCode, routing::get};
use tokio::sync::watch;

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
    /// Last stream sequence this instance has applied. Projectors publish here;
    /// request handlers wait on it for read-your-own-writes.
    applied: watch::Sender<u64>,
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
                            applied: watch::channel(0).0,
                        },
                    )
                })
                .collect(),
        })
    }

    /// Called by a projector once it has reached the stream's last sequence.
    pub fn mark_caught_up(&self, stream: &str) {
        if let Some(s) = self.streams.get(stream) {
            s.caught_up.store(true, Ordering::Release);
            tracing::info!(stream, "caught up");
        }
    }

    /// Called when a projector hits an error it can't apply.
    ///
    /// A projector must stop rather than skip the message: skipping makes this
    /// instance permanently disagree with its peers, which is far worse than
    /// being drained. Flipping readiness is how it gets taken out of rotation.
    pub fn mark_failed(&self, stream: &str) {
        if let Some(s) = self.streams.get(stream) {
            s.caught_up.store(false, Ordering::Release);
            tracing::error!(stream, "projector stalled — instance not ready");
        }
    }

    /// Records progress after a message has been applied *and committed*.
    pub fn set_applied(&self, stream: &str, seq: u64) {
        if let Some(s) = self.streams.get(stream) {
            let _ = s.applied.send(seq);
        }
    }

    /// Watch handle for a stream's applied sequence.
    pub fn applied_rx(&self, stream: &str) -> Option<watch::Receiver<u64>> {
        self.streams.get(stream).map(|s| s.applied.subscribe())
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
    let ready = readiness.clone();
    Router::new()
        // Liveness: deliberately does not consult NATS. If this returned 503 while
        // the broker was down, an orchestrator would restart a perfectly healthy
        // process and achieve nothing.
        .route("/healthz", get(|| async { "ok" }))
        .route(
            "/readyz",
            get(move || {
                let ready = ready.clone();
                async move {
                    match ready.status() {
                        Ok(()) => (StatusCode::OK, "ready".to_string()),
                        Err(reason) => (StatusCode::SERVICE_UNAVAILABLE, reason),
                    }
                }
            }),
        )
}
