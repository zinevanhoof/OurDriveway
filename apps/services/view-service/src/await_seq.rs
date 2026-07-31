use std::{collections::HashMap, sync::Arc, time::Duration};

use axum::{extract::Request, http::HeaderName, middleware::Next, response::Response};
use tokio::sync::watch;

/// Header a client echoes back after a write: `X-Await-Seq: SPOTS:4712`.
pub const AWAIT_SEQ: HeaderName = HeaderName::from_static("x-await-seq");

/// Read-your-own-writes for a system where writes are published, not written.
///
/// A write returns 202 with the stream sequence its event landed at. Because the
/// projections that answer reads are per-instance and eventually consistent, an
/// immediate follow-up read can legitimately land on an instance that hasn't
/// applied that event — the user creates a spot and it isn't in the list.
///
/// The client echoes the sequence on its next read and this layer waits for the
/// local projector to reach it. One place, covering every write path, instead of
/// a refetch-and-hope in each caller.
#[derive(Clone)]
pub struct AppliedSeqs(pub Arc<HashMap<&'static str, watch::Receiver<u64>>>);

pub async fn await_seq(
    axum::extract::State(seqs): axum::extract::State<AppliedSeqs>,
    request: Request,
    next: Next,
) -> Response {
    if let Some(want) = request.headers().get(AWAIT_SEQ).and_then(parse) {
        if let Some(rx) = seqs.0.get(want.0.as_str()) {
            let mut rx = rx.clone();
            // Capped: a stalled projector must not hang the request. Proceeding
            // with slightly stale data beats hanging — and /readyz will already
            // have taken a genuinely stuck instance out of rotation.
            let _ = tokio::time::timeout(Duration::from_secs(2), async {
                while *rx.borrow() < want.1 {
                    if rx.changed().await.is_err() {
                        break;
                    }
                }
            })
            .await;
        }
    }
    next.run(request).await
}

/// `"SPOTS:4712"` -> `("SPOTS", 4712)`. Anything malformed is ignored rather than
/// rejected: this is an optimisation, not an authorization input.
fn parse(value: &axum::http::HeaderValue) -> Option<(String, u64)> {
    let text = value.to_str().ok()?;
    let (stream, seq) = text.split_once(':')?;
    Some((stream.to_string(), seq.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn parses_and_shrugs_off_junk() {
        assert_eq!(
            parse(&HeaderValue::from_static("SPOTS:4712")),
            Some(("SPOTS".into(), 4712))
        );
        assert_eq!(parse(&HeaderValue::from_static("SPOTS")), None);
        assert_eq!(parse(&HeaderValue::from_static("SPOTS:abc")), None);
        assert_eq!(parse(&HeaderValue::from_static("")), None);
    }
}
