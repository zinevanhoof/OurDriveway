use std::{collections::HashMap, sync::Arc, time::Duration};

use axum::{extract::Request, http::HeaderName, middleware::Next, response::Response};
use tokio::sync::watch;

/// Header a client echoes back after a write: `X-Await-Seq: SPOTS:4712`.
///
/// Carries one position per stream the client has written to, comma-separated —
/// `SPOTS:4712,SESSIONS:19`. A client that only ever writes one stream sends the
/// single-entry form, which is why this stayed one header rather than becoming a
/// list of them: the common case is unchanged.
pub const AWAIT_SEQ: HeaderName = HeaderName::from_static("x-await-seq");

/// `("SPOTS", 4712)` -> `"SPOTS:4712"`. The inverse of [`parse`].
///
/// The write half of this file's protocol, kept next to the read half so the two
/// cannot drift — see the round-trip test below. The *shape* a write answers with
/// is not here: that is `shared::responses::common`, because a response body is
/// not bus plumbing and this crate depends on `shared` rather than the reverse. A
/// route composes them: `accepted(format_seq(STREAM_SPOTS, seq))`.
pub fn format_seq(stream: &str, seq: u64) -> String {
    format!("{stream}:{seq}")
}

/// Default wait for a projection, used whenever [`await_applied`] is passed `None`.
///
/// Capped: a stalled projector must not hang a request. Proceeding with slightly
/// stale data beats hanging — and `/readyz` will already have taken a genuinely
/// stuck instance out of rotation.
pub const TIMEOUT: Duration = Duration::from_secs(2);

/// Blocks until this instance's projector has applied `seq`, or `timeout` elapses.
///
/// The other half of read-your-own-writes, for the service that *published* the
/// event rather than the client that will read it back: a handler publishes,
/// waits here, and returns — so a signup immediately followed by a login sees its
/// own write without the client having to echo a header.
///
/// `timeout` of `None` means [`TIMEOUT`]. Rust has no default arguments, and an
/// `Option` the callee unwraps is the nearest thing — so the common caller writes
/// `None` and never has to know the number.
///
/// Returns whether `seq` was actually reached, `false` also covering the projector
/// having stopped: its sender is dropped, so there is nothing left to wait for. Most
/// callers ignore it — for a request handler a miss is a normal outcome, not a fault.
/// A caller about to *act* on the projection rather than read it back checks it:
/// payment-service's workers decide whether to move money, and a NAK is their retry.
pub async fn await_applied(rx: &watch::Receiver<u64>, seq: u64, timeout: Option<Duration>) -> bool {
    let mut rx = rx.clone();
    tokio::time::timeout(timeout.unwrap_or(TIMEOUT), async {
        while *rx.borrow() < seq {
            if rx.changed().await.is_err() {
                return false;
            }
        }
        true
    })
    .await
    .unwrap_or(false)
}

/// Read-your-own-writes for a system where writes are published, not written,
/// driven by the client rather than by the handler.
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
    if let Some(header) = request.headers().get(AWAIT_SEQ) {
        let wanted: Vec<_> = parse(header)
            .filter_map(|(stream, seq)| seqs.0.get(stream.as_str()).map(|rx| (rx, seq)))
            .collect();

        // One budget for the whole header, not [`TIMEOUT`] per stream: a client
        // that has written three streams must not be able to hold a request open
        // for three times as long as one that wrote one.
        let _ = tokio::time::timeout(TIMEOUT, async {
            for (rx, seq) in wanted {
                await_applied(rx, seq, Some(TIMEOUT)).await;
            }
        })
        .await;
    }
    next.run(request).await
}

/// `"SPOTS:4712,SESSIONS:19"` -> `[("SPOTS", 4712), ("SESSIONS", 19)]`.
///
/// Malformed entries are skipped rather than rejecting the header: this is an
/// optimisation, not an authorization input, and one junk entry must not cost a
/// client the positions it got right.
fn parse(value: &axum::http::HeaderValue) -> impl Iterator<Item = (String, u64)> {
    value
        .to_str()
        .unwrap_or_default()
        .split(',')
        .filter_map(|entry| {
            let (stream, seq) = entry.trim().split_once(':')?;
            Some((stream.to_string(), seq.parse().ok()?))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn parsed(header: &'static str) -> Vec<(String, u64)> {
        parse(&HeaderValue::from_static(header)).collect()
    }

    #[test]
    fn parses_and_shrugs_off_junk() {
        assert_eq!(parsed("SPOTS:4712"), [("SPOTS".to_string(), 4712)]);
        assert!(parsed("SPOTS").is_empty());
        assert!(parsed("SPOTS:abc").is_empty());
        assert!(parsed("").is_empty());
    }

    /// A client that has written more than one stream needs to wait on all of
    /// them, and one bad entry must not cost it the good ones — the header is an
    /// optimisation, so the failure mode has to be "wait for less", never "reject".
    #[test]
    fn every_stream_in_the_header_is_waited_on() {
        assert_eq!(
            parsed("SPOTS:4712,SESSIONS:19"),
            [("SPOTS".to_string(), 4712), ("SESSIONS".to_string(), 19)]
        );
        assert_eq!(
            parsed("SPOTS:4712,junk,USERS:8"),
            [("SPOTS".to_string(), 4712), ("USERS".to_string(), 8)]
        );
    }

    /// The write side and the read side of the same string, in one assertion: a
    /// service formats a position into its 202, the client echoes it, and this
    /// file parses it back. Nothing else checks that those two agree.
    #[test]
    fn what_a_write_answers_is_what_the_layer_parses() {
        let seq = format_seq("SPOTS", 4712);
        assert_eq!(
            parse(&HeaderValue::from_str(&seq).unwrap()).collect::<Vec<_>>(),
            [("SPOTS".to_string(), 4712)]
        );
    }
}
