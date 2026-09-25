use async_nats::ServerAddr;
use async_nats::jetstream::{self, Context, stream::Config};
use shared::error::myerror::{MyError, MyResult};
use shared::events::STREAMS;

/// Connects to NATS and returns a JetStream context.
///
/// The URL is passed in from the caller's `Config`. It used to default to
/// `nats://localhost:4222` when unset — which in a container meant a service
/// quietly dialled itself, failed, and looked like a broker outage.
///
/// `nats://user:pass@host:4222` is accepted, so a service still reads one variable —
/// but the credentials are lifted out and handed over as options, because async-nats
/// 0.50 **ignores** a password in the URL: its connector only reads `ConnectOptions`,
/// and against a server with authorization on, the URL form is refused as anonymous.
pub async fn connect(url: &str) -> MyResult<Context> {
    let (address, credentials) = split_credentials(url)?;

    let mut options = async_nats::ConnectOptions::new();
    if let Some((user, password)) = credentials {
        options = options.user_and_password(user, password);
    }

    // `address`, never `url`, in anything that is logged or returned: the URL carries
    // the password.
    let client = options
        .connect(address.as_str())
        .await
        .map_err(|e| MyError::Bus(format!("connect {address}: {e}")))?;
    tracing::info!(url = %address, "connected to NATS");
    Ok(jetstream::new(client))
}

/// `nats://user:pass@host:4222` -> (`nats://host:4222`, `Some((user, pass))`).
fn split_credentials(url: &str) -> MyResult<(String, Option<(String, String)>)> {
    // The parse error is not formatted in: it may quote the input, password and all.
    let addr: ServerAddr = url
        .parse()
        .map_err(|_| MyError::Bus("NATS_URL is not a valid URL".into()))?;

    let address = format!("{}://{}:{}", addr.scheme(), addr.host(), addr.port());
    let credentials = addr
        .username()
        .map(|user| (user.to_string(), addr.password().unwrap_or_default().to_string()));
    Ok((address, credentials))
}

/// Per-stream ceiling on disk. The second half of a retention limit, and the half
/// that holds when the first one doesn't: `max_age` bounds a *normal* week, this
/// bounds a bug — a loop that republishes, or a relay that never deletes.
///
/// Old messages are discarded to stay under it, which is the right failure for an
/// integration bus and would have been the wrong one while the log was
/// authoritative. That is the trade the whole of step 5 makes.
const MAX_BYTES: i64 = 1024 * 1024 * 1024;

/// Declares the streams. Safe to call from every instance concurrently and on every
/// boot.
///
/// Every stream binds `<domain>.>`, so it claims every subject in its bounded
/// context regardless of the entity keying below it, and one durable consumer per
/// projector reads the whole of it.
///
/// `create_or_update_stream`, not `get_or_create_stream`. The latter takes an
/// existing stream as-is and ignores everything below it, so `max_age`, `max_bytes`
/// and `duplicate_window` were only ever applied to streams that did not exist yet —
/// `nats stream edit` by hand was the real way to change one. This makes the code the
/// source of truth instead, and a field NATS treats as immutable now fails loudly
/// rather than being silently dropped.
///
/// ## The partition transform is gone
///
/// These streams used to carry a `subject_transform` rewriting
/// `<domain>.<entity>.<id>` to `<domain>.<0..15>.<entity>.<id>` on ingest, so that
/// sixteen consumers could each filter one lane and be individually ordered by
/// `max_ack_pending: 1`. All of that existed to deliver one aggregate's events in
/// order, and `bus::projector::decide` does that from the stored version now.
///
/// It came with a migration note that no longer applies: changing the partition count
/// required draining every `-pNN` durable first, because stored messages keep the
/// subject they were written with and a key that moved lanes had its history split
/// across two independently-ordered consumers.
///
/// Removing it is safe in place. `create_or_update_stream` applies
/// `subject_transform: None` to the live stream, new messages are stored with their
/// published three-token subject, already-stored ones keep their four-token one, and
/// `<domain>.>` matches both — `>` matches one *or more* trailing tokens.
pub async fn ensure_streams(js: &Context) -> MyResult<()> {
    for (name, domain, max_age) in STREAMS {
        js.create_or_update_stream(Config {
            name: (*name).to_string(),
            subjects: vec![shared::events::stream_filter(domain)],
            // File storage, still — a week of events outlives any single node's
            // memory and a consumer that was down overnight must find them. Not
            // because the stream is the source of truth; TiKV is.
            storage: jetstream::stream::StorageType::File,
            retention: jetstream::stream::RetentionPolicy::Limits,
            max_age: *max_age,
            max_bytes: MAX_BYTES,
            // Publishers set Nats-Msg-Id to the event id, so a retried publish
            // within this window is discarded instead of duplicating the event.
            //
            // An hour, not the two minutes this used to be. The relay is
            // at-least-once by construction — it publishes, then deletes the row,
            // and a crash in that gap republishes on restart — so this window is
            // what decides whether that tail reaches consumers at all. Two minutes
            // covered a stall; it did not cover a pod restart, a rollout, or a node
            // drain, and outside it the event genuinely landed twice.
            //
            // It is also what makes running a relay on EVERY replica cheap rather
            // than merely correct: two relays reading the same batch publish the
            // same event ids, and the server drops the second copy instead of
            // waking every consumer with it.
            //
            // Cost is JetStream holding an id and a timestamp per published message
            // for the window. Negligible here, and the first number to revisit if
            // NATS memory ever becomes interesting.
            duplicate_window: std::time::Duration::from_secs(60 * 60),
            ..Default::default()
        })
        .await
        .map_err(|e| MyError::Bus(format!("ensure stream {name}: {e}")))?;
        tracing::info!(stream = name, domain, "stream ready");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_leave_the_url_that_gets_logged() {
        let (address, credentials) =
            split_credentials("nats://ourdriveway:0123abcd@ourdriveway-nats:4222").unwrap();
        assert_eq!(address, "nats://ourdriveway-nats:4222");
        assert_eq!(
            credentials,
            Some(("ourdriveway".to_string(), "0123abcd".to_string()))
        );
    }

    #[test]
    fn a_url_without_credentials_connects_anonymously() {
        let (address, credentials) = split_credentials("nats://127.0.0.1:4222").unwrap();
        assert_eq!(address, "nats://127.0.0.1:4222");
        assert_eq!(credentials, None);
    }
}
