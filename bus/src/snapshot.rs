use std::{collections::HashMap, sync::Arc, time::Duration};

use async_nats::jetstream::{
    Context,
    object_store::{Config, ObjectMetadata, ObjectStore},
};
use bytes::Bytes;
use futures::TryStreamExt;
use shared::error::myerror::{MyError, MyResult};
use surrealdb::{Surreal, engine::remote::http::Client as HttpClient};
use tokio::io::AsyncWriteExt;
use tokio_util::io::StreamReader;

/// Bucket holding one snapshot object per service.
const BUCKET: &str = "snapshots";

/// Periodic database snapshots, so a cold start doesn't replay the whole log.
///
/// Warm restarts don't need this at all — the projection volume survives and the
/// cursor resumes. Snapshots are purely about the *cold* case: a new instance, or
/// one whose volume was wiped, would otherwise replay from sequence 1 forever
/// more slowly as the log grows.
///
/// Uses a second connection because `export`/`import` are **HTTP-engine only**
/// (the WebSocket engine has no Backup support). The app keeps its `Surreal<Ws>`.
pub struct Snapshotter {
    pub db: Surreal<HttpClient>,
    pub store: ObjectStore,
    /// Object name — one per service, e.g. "spot-service".
    pub name: String,
    /// Streams whose cursors this service owns, e.g. `["USERS", "SESSIONS"]`.
    pub streams: Vec<&'static str>,
}

/// Everything the snapshotter needs, all of it from the calling service's own
/// `Config`.
///
/// A struct rather than seven positional arguments: `db_addr`, `db_user`,
/// `db_pass` and `service` are four adjacent `&str`, and getting two of them the
/// wrong way round would produce a runtime auth failure that reads like a
/// network problem.
pub struct SnapshotConfig<'a> {
    pub db_addr: &'a str,
    pub db_user: &'a str,
    pub db_pass: &'a str,
    /// Object name — one per service, e.g. "spot-service".
    pub service: &'a str,
    /// Streams whose cursors this service owns, e.g. `["USERS", "SESSIONS"]`.
    pub streams: Vec<&'static str>,
    /// Seconds between snapshots. **0 disables the whole path**, which is how
    /// this runs against a disposable projection store.
    pub every_secs: u64,
}

/// Connects the HTTP engine and ensures the bucket exists.
async fn connect(js: &Context, cfg: &SnapshotConfig<'_>) -> MyResult<Snapshotter> {
    let db = Surreal::new::<surrealdb::engine::remote::http::Http>(cfg.db_addr)
        .await
        .map_err(|e| MyError::Bus(format!("snapshot http connect: {e}")))?;
    db.signin(surrealdb::opt::auth::Root {
        username: cfg.db_user.to_string(),
        password: cfg.db_pass.to_string(),
    })
    .await
    .map_err(|e| MyError::Bus(format!("snapshot signin: {e}")))?;
    db.use_ns("main")
        .use_db("main")
        .await
        .map_err(|e| MyError::Bus(format!("snapshot use ns/db: {e}")))?;

    let store = match js.get_object_store(BUCKET).await {
        Ok(store) => store,
        Err(_) => js
            .create_object_store(Config {
                bucket: BUCKET.to_string(),
                description: Some("Projection snapshots, keyed by service".into()),
                ..Default::default()
            })
            .await
            .map_err(|e| MyError::Bus(format!("create object store: {e}")))?,
    };

    Ok(Snapshotter {
        db,
        store,
        name: cfg.service.to_string(),
        streams: cfg.streams.clone(),
    })
}

impl Snapshotter {
    /// Restores from the latest snapshot if this projection is empty.
    ///
    /// Returns `true` if a snapshot was applied. Only ever runs against an empty
    /// projection: importing over live data would resurrect rows the log has
    /// since removed.
    pub async fn restore_if_empty(&self) -> MyResult<bool> {
        if self.cursor_count().await? > 0 {
            return Ok(false);
        }
        let mut object = match self.store.get(&self.name).await {
            Ok(o) => o,
            Err(_) => {
                tracing::info!(name = %self.name, "no snapshot to restore; replaying from the start");
                return Ok(false);
            }
        };
        let info = self
            .store
            .info(&self.name)
            .await
            .map_err(|e| MyError::Bus(format!("snapshot info: {e}")))?;

        // import() takes a path, not a byte stream, so the object lands on disk.
        let path = std::env::temp_dir().join(format!("{}-restore.surql", self.name));
        let mut file = tokio::fs::File::create(&path)
            .await
            .map_err(|e| MyError::Bus(format!("snapshot temp file: {e}")))?;
        tokio::io::copy(&mut object, &mut file)
            .await
            .map_err(|e| MyError::Bus(format!("snapshot download: {e}")))?;
        file.flush()
            .await
            .map_err(|e| MyError::Bus(format!("snapshot flush: {e}")))?;
        drop(file);

        self.db
            .import(&path)
            .await
            .map_err(|e| MyError::Bus(format!("snapshot import: {e}")))?;
        let _ = tokio::fs::remove_file(&path).await;

        // Overwrite the imported cursors with the ones recorded *before* the
        // export ran. An export is not a point-in-time transaction, so a write
        // landing mid-export can leave `_projection` newer than some of the rows
        // beside it — and a cursor that is too new SKIPS events, which is silent
        // corruption. A cursor that is too old merely replays, and every apply is
        // an idempotent UPSERT.
        for stream in &self.streams {
            if let Some(seq) = info.metadata.get(*stream).and_then(|v| v.parse::<i64>().ok()) {
                self.db
                    .query("UPSERT type::record('_projection', $s) SET last_seq = $seq, updated_at = time::now()")
                    .bind(("s", (*stream).to_string()))
                    .bind(("seq", seq))
                    .await
                    .map_err(|e| MyError::Bus(format!("restore cursor {stream}: {e}")))?
                    .check()
                    .map_err(|e| MyError::Bus(format!("restore cursor {stream}: {e}")))?;
            }
        }

        tracing::info!(name = %self.name, bytes = info.size, cursors = ?info.metadata, "restored from snapshot");
        Ok(true)
    }

    /// Exports the database straight into the object store, recording each
    /// stream's cursor as it was *before* the export began.
    ///
    /// Uses the byte-stream form of `export` rather than exporting to a file and
    /// re-reading it, so the snapshot never touches local disk on the way out:
    ///
    /// ```text
    ///   file form:  DB -> HTTP -> write file -> read file -> chunk -> NATS
    ///   this:       DB -> HTTP -> channel ─────────────────> chunk -> NATS
    /// ```
    ///
    /// Memory stays flat regardless of database size: SurrealDB forwards the HTTP
    /// body through a capacity-1 channel, so at most one chunk is in flight and a
    /// slow upload back-pressures the export rather than buffering ahead of it.
    ///
    /// The restore path still uses a file, because `import` only accepts a path.
    pub async fn take(&self) -> MyResult<()> {
        let mut cursors = HashMap::new();
        for stream in &self.streams {
            let seq: Option<i64> = self
                .db
                .query("SELECT VALUE last_seq FROM ONLY type::record('_projection', $s)")
                .bind(("s", (*stream).to_string()))
                .await
                .map_err(|e| MyError::Bus(format!("read cursor {stream}: {e}")))?
                .take(0)
                .map_err(|e| MyError::Bus(format!("read cursor {stream}: {e}")))?;
            cursors.insert((*stream).to_string(), seq.unwrap_or(0).to_string());
        }

        // Data only. Every database already applies its own schema at boot via
        // `--import-file /schema.surql`, so exporting definitions re-does work
        // that is already done — and drags along statements that don't survive
        // the round trip.
        //
        // `configs` is the one that actually bites: SurrealDB 3.2.1 serialises a
        // GraphQL config as `GRAPHQL TABLES AUTO FUNCTIONS AUTO;`, dropping the
        // `DEFINE CONFIG` prefix, and its own parser then rejects the line. That
        // made every cold restore of view-service — the only database with a
        // GraphQL config — fail outright, which is strictly worse than having no
        // snapshot, because the service refuses to boot instead of quietly
        // replaying from sequence 1.
        //
        // The rest are off for the same reason in principle, and because a
        // snapshot has no business restoring database users or access rules.
        let backup = self
            .db
            .export(())
            .with_config()
            .configs(false)
            .users(false)
            .accesses(false)
            .params(false)
            .functions(false)
            .analyzers(false)
            // Tables stay on: `records` are emitted per table and are skipped
            // entirely if the table isn't included. The `DEFINE TABLE` that comes
            // with them is redundant but harmless — see restore_if_empty.
            .tables(true)
            .records(true)
            .await
            .map_err(|e| MyError::Bus(format!("snapshot export: {e}")))?;

        // Backup yields Result<Vec<u8>>; StreamReader needs a Buf and an io::Error.
        let mut reader = StreamReader::new(
            backup
                .map_ok(Bytes::from)
                .map_err(|e| std::io::Error::other(e.to_string())),
        );

        let info = self
            .store
            .put(
                ObjectMetadata {
                    name: self.name.clone(),
                    description: Some("Projection snapshot".into()),
                    metadata: cursors.clone(),
                    ..Default::default()
                },
                &mut reader,
            )
            .await
            .map_err(|e| MyError::Bus(format!("snapshot put: {e}")))?;

        tracing::info!(name = %self.name, bytes = info.size, cursors = ?cursors, "snapshot stored");
        Ok(())
    }

    /// Counted in Rust rather than with `count() ... GROUP ALL`, whose result is
    /// an object and needs unwrapping. `_projection` holds one row per stream, so
    /// there is nothing to optimise here.
    async fn cursor_count(&self) -> MyResult<usize> {
        let rows: Vec<String> = self
            .db
            .query("SELECT VALUE record::id(id) FROM _projection")
            .await
            .map_err(|e| MyError::Bus(format!("cursor count: {e}")))?
            .take(0)
            .map_err(|e| MyError::Bus(format!("cursor count: {e}")))?;
        Ok(rows.len())
    }
}

/// Takes a snapshot on a jittered interval, forever. Reached only via `install`,
/// which is what decides whether snapshots run at all.
///
/// ponytail: every instance snapshots and the last write wins. They are all
/// projections of the same log, so whichever object survives is valid — only the
/// upload bandwidth is wasted. Add a KV lock or a single snapshotter deployment
/// if that ever matters.
///
/// The jitter keeps N instances from uploading in lockstep after a simultaneous
/// deploy.
fn spawn(snapshotter: Arc<Snapshotter>, every: Duration) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(jittered(every, std::process::id())).await;

            if let Err(e) = snapshotter.take().await {
                // A failed snapshot is not fatal: the log is still the source of
                // truth, so the only cost is a longer cold start.
                tracing::warn!(error = %e, "snapshot failed; will retry next interval");
            }
        }
    });
}

/// Restore on boot, then snapshot forever — or do nothing at all.
///
/// The whole snapshot path is one call so that "off" is one decision in one
/// place. Off is a legitimate steady state, not a degraded one: against a
/// disposable projection store every start is a cold start anyway, and replaying
/// a few thousand events costs less than the object-store round trip. Raise
/// `every_secs` once the log is long enough for replay to be felt.
pub async fn install(js: &Context, cfg: SnapshotConfig<'_>) -> MyResult<()> {
    if cfg.every_secs == 0 {
        tracing::info!(
            service = cfg.service,
            "snapshots disabled; projections replay from the log"
        );
        return Ok(());
    }

    // Cold start only: restore from the newest snapshot before the projectors
    // begin, so replay resumes from the snapshot's cursor instead of sequence 1.
    let snapshotter = Arc::new(connect(js, &cfg).await?);
    snapshotter.restore_if_empty().await?;
    spawn(snapshotter, Duration::from_secs(cfg.every_secs));
    Ok(())
}

/// `every` scaled by ±20%, derived from the process id so replicas spread out
/// without pulling in a random number generator.
fn jittered(every: Duration, seed: u32) -> Duration {
    let offset = (seed % 41) as f64 - 20.0; // -20 ..= +20
    every.mul_f64(1.0 + offset / 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jitter_stays_within_twenty_percent() {
        let base = Duration::from_secs(900);
        for seed in 0..500u32 {
            let d = jittered(base, seed);
            assert!(
                d >= base.mul_f64(0.8) && d <= base.mul_f64(1.2),
                "seed {seed} produced {d:?}, outside ±20% of {base:?}"
            );
        }
        // Different processes must not line up, or a simultaneous deploy has
        // every replica uploading at the same moment.
        assert_ne!(jittered(base, 1), jittered(base, 2));
    }
}
