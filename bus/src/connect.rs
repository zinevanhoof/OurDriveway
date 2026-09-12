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
/// context regardless of the entity keying below it, and carries a
/// [`shared::events::partition_transform`] that rewrites
/// `<domain>.<entity>.<id>` to `<domain>.<partition>.<entity>.<id>` **as it stores
/// each message**. That is the whole of the partitioning: publishers are unchanged,
/// consumers filter on one partition, and nothing in this codebase hashes anything.
///
/// `create_or_update_stream`, not `get_or_create_stream`. The latter takes an
/// existing stream as-is and ignores everything below it, so `max_age`, `max_bytes`
/// and `duplicate_window` were only ever applied to streams that did not exist yet —
/// `nats stream edit` by hand was the real way to change one. This makes the code the
/// source of truth instead, and a field NATS treats as immutable now fails loudly
/// rather than being silently dropped.
///
/// It matters more since the transform: a stream without one stores three-token
/// subjects that no lane's `<domain>.<partition>.>` filter matches, so every consumer
/// would sit at zero pending for ever with nothing logged.
///
/// ## Changing `PARTITIONS` still needs a drain
///
/// This rewrites the transform on the live stream; it cannot re-shard what is already
/// stored. Those messages keep the subject they were written with, so a key that
/// moves lanes has its older events in the old lane and its newer ones in the new
/// lane — two independent consumers, each `max_ack_pending: 1`, with no order between
/// them. Shrinking the count is worse: anything stored above the new ceiling has no
/// consumer at all, and the durables above it keep a backlog nobody drains.
///
/// So: get every `-pNN` durable to 0 pending (`nats consumer report <STREAM>`), then
/// deploy. Drained, there is nothing left in the old lanes to race. Projectors
/// normally sit at the head, so this is a check rather than a wait.
///
/// One residue: drained is not deleted. Old-subject messages stay for the rest of
/// `max_age`, and a durable created *after* the change with [`DeliverPolicy::All`]
/// — a renamed projector, a new service — filters them out rather than replaying
/// them. Deleting the streams avoids that and costs seven days of integration
/// events, which TiKV is authoritative over anyway.
///
/// [`DeliverPolicy::All`]: async_nats::jetstream::consumer::DeliverPolicy::All
pub async fn ensure_streams(js: &Context) -> MyResult<()> {
    for (name, domain, max_age) in STREAMS {
        let (source, destination) = shared::events::partition_transform(domain);
        js.create_or_update_stream(Config {
            name: (*name).to_string(),
            subjects: vec![shared::events::stream_filter(domain)],
            // Applied on ingest, after `subjects` has matched. Which is why that
            // filter stays `<domain>.>` rather than narrowing to the transform's
            // source: narrowing it would reject the very messages this partitions.
            subject_transform: Some(jetstream::stream::SubjectTransform {
                source,
                destination,
            }),
            // File storage, still — a week of events outlives any single node's
            // memory and a consumer that was down overnight must find them. Not
            // because the stream is the source of truth; TiKV is.
            storage: jetstream::stream::StorageType::File,
            retention: jetstream::stream::RetentionPolicy::Limits,
            max_age: *max_age,
            max_bytes: MAX_BYTES,
            // Publishers set Nats-Msg-Id to the event id, so a retried publish
            // within this window is discarded instead of duplicating the event.
            duplicate_window: std::time::Duration::from_secs(120),
            ..Default::default()
        })
        .await
        .map_err(|e| MyError::Bus(format!("ensure stream {name}: {e}")))?;
        tracing::info!(
            stream = name,
            domain,
            partitions = shared::events::PARTITIONS,
            "stream ready"
        );
    }
    Ok(())
}
