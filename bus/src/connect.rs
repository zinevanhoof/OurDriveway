use async_nats::jetstream::{self, Context, stream::Config};
use shared::events::STREAMS;

/// Connects to NATS and returns a JetStream context.
///
/// `NATS_URL` defaults to the compose service name so a service started with no
/// configuration in dev behaves the same as in a container.
pub async fn connect() -> Result<Context, async_nats::Error> {
    let url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://localhost:4222".into());
    let client = async_nats::connect(&url).await?;
    tracing::info!(%url, "connected to NATS");
    Ok(jetstream::new(client))
}

/// Creates the streams if they don't exist. Safe to call from every instance
/// concurrently and on every boot — `get_or_create_stream` is idempotent.
///
/// Every stream binds `<domain>.*.>`, so raising `SHARD_COUNT` never requires a
/// stream change.
pub async fn ensure_streams(js: &Context) -> Result<(), async_nats::Error> {
    for (name, subject, max_age) in STREAMS {
        js.get_or_create_stream(Config {
            name: (*name).to_string(),
            subjects: vec![(*subject).to_string()],
            // File storage because these streams are the source of truth, not a
            // transport buffer. Retention is by limits; the ones with no max_age
            // are meant to be replayed from sequence 1 forever.
            storage: jetstream::stream::StorageType::File,
            retention: jetstream::stream::RetentionPolicy::Limits,
            max_age: max_age.unwrap_or_default(),
            // Publishers set Nats-Msg-Id to the event id, so a retried publish
            // within this window is discarded instead of duplicating the event.
            duplicate_window: std::time::Duration::from_secs(120),
            ..Default::default()
        })
        .await?;
        tracing::info!(stream = name, subject, "stream ready");
    }
    Ok(())
}
