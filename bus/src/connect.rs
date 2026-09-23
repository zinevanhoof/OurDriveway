use async_nats::jetstream::{self, Context, stream::Config};
use shared::error::myerror::{MyError, MyResult};
use shared::events::STREAMS;

/// Connects to NATS and returns a JetStream context.
///
/// The URL is passed in from the caller's `Config`. It used to default to
/// `nats://localhost:4222` when unset — which in a container meant a service
/// quietly dialled itself, failed, and looked like a broker outage.
pub async fn connect(url: &str) -> MyResult<Context> {
    let client = async_nats::connect(url)
        .await
        .map_err(|e| MyError::Bus(format!("connect {url}: {e}")))?;
    tracing::info!(%url, "connected to NATS");
    Ok(jetstream::new(client))
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
