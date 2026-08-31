//! Read-your-own-writes, keyed by aggregate rather than by log position.
//!
//! A write answers `202 { seq: "spot:019f…@3" }`. Reads are served from
//! projections that lag the write by however long the outbox relay and the
//! projector take, so an immediate follow-up read can legitimately land before its
//! own write is visible — you create a spot and it is not in the list.
//!
//! The client echoes that token on its next read and this layer holds the request
//! until the projection has reached it.
//!
//! ## Why not the stream sequence
//!
//! This replaced a header carrying `SPOTS:4712`, and the change was forced rather
//! than chosen. Once TiKV is authoritative a write commits *before* its event
//! reaches NATS — the outbox relay publishes afterwards — so at the moment a route
//! has to answer, no stream sequence exists yet. The aggregate's version does: it
//! was assigned inside the transaction that committed.
//!
//! It is also narrower in a way that matters. `SPOTS:4712` means "wait until this
//! projection has applied every spot event up to 4712", including hundreds
//! belonging to other people's spots. `spot:<id>@3` waits for one row.
//!
//! ## It answers the consumer's question too, now
//!
//! There used to be a second mechanism beside this one — `bus::await_applied`,
//! waiting on a *stream* position — for payment-service's workers, whose question
//! was "has my projection consumed the event that woke me". It read
//! `ack_floor.stream_sequence` off a stream's single consumer.
//!
//! That number stopped having one value when each stream grew `PARTITIONS` cursors:
//! the lane holding the event can be current while an unrelated lane sits lower, and
//! a minimum across them would stall a refund behind a partition the worker does not
//! care about. It was always the coarser question anyway — a worker holds one event
//! about one aggregate. So the two collapsed into [`reached`], and this module
//! answers both.

use std::time::Duration;

use axum::{extract::Request, http::HeaderName, middleware::Next, response::Response};
use shared::events::{parse_version, split_aggregate};
use sqlx::PgPool;
use uuid::Uuid;

/// Header a client echoes back after a write: `X-Await-Version: spot:019f…@3`.
///
/// Carries one entry per aggregate the client has written, comma-separated.
pub const AWAIT_VERSION: HeaderName = HeaderName::from_static("x-await-version");

/// Ceiling for the whole header, not per entry: a client that has written three
/// aggregates must not be able to hold a request open three times as long.
///
/// Proceeding slightly stale beats hanging. A genuinely stuck projector is
/// `/readyz`'s problem, not this request's.
pub const TIMEOUT: Duration = Duration::from_secs(2);

/// Gap between checks while behind. Short enough to be invisible next to the
/// round trip the client is already making; long enough not to spin.
const POLL: Duration = Duration::from_millis(25);

/// The database this service serves reads from — the same one its projectors
/// write. Wrapped so it is a distinct extractor from any other `Surreal` state.
#[derive(Clone)]
pub struct AwaitVersions(pub PgPool);

pub async fn await_version(
    axum::extract::State(db): axum::extract::State<AwaitVersions>,
    request: Request,
    next: Next,
) -> Response {
    if let Some(header) = request.headers().get(AWAIT_VERSION) {
        let wanted: Vec<_> = parse(header.to_str().unwrap_or_default()).collect();

        let _ = tokio::time::timeout(TIMEOUT, async {
            for (table, id, want) in wanted {
                wait_one(&db.0, &table, &id, want).await;
            }
        })
        .await;
    }
    next.run(request).await
}

/// Blocks until `aggregate:id` has reached `want`, or the caller's timeout fires.
async fn wait_one(db: &PgPool, table: &str, id: &Uuid, want: u64) {
    // The outer [`TIMEOUT`] in the middleware caps the whole header, so this one is
    // only a backstop for a single entry.
    let _ = reached(db, table, id, want, TIMEOUT).await;
}

/// Whether `table:id` reached `want` within `timeout`.
///
/// The same question `wait_one` asks for a client's `X-Await-Version` header,
/// exposed because a **consumer** needs it too: payment-service's `BookingWorker` is
/// woken by a BOOKINGS event and must not decide whether to move money until its own
/// mirror of that booking includes it.
///
/// This replaced `bus::await_applied`, which waited on a stream sequence. That
/// number stopped having a single value once each stream grew `PARTITIONS` cursors —
/// and it was always the coarser question, since a worker holding one event cares
/// about one aggregate rather than about everything published before it.
pub async fn reached(
    db: &PgPool,
    aggregate: &str,
    id: &Uuid,
    want: u64,
    timeout: Duration,
) -> bool {
    // Resolved once, outside the loop: an aggregate name no service owns is a caller
    // bug, not something to retry until the timeout. Also the only thing standing
    // between a client-supplied token and a table name spliced into SQL — Postgres
    // cannot bind an identifier, so this must never be interpolated unchecked.
    let Some(table) = shared::db::table_for(aggregate) else {
        return false;
    };
    let sql = format!("SELECT version FROM {table} WHERE id = $1");

    tokio::time::timeout(timeout, async {
        loop {
            match current(db, &sql, id).await {
                Ok(Some(have)) if have >= want => return true,
                // The row is not there yet — its creating event has not been applied
                // — so keep waiting. This is the common case for a read that follows
                // a create closely.
                Ok(_) => {}
                // The table does not exist in *this* database, which is not a fault:
                // view-service holds `payout` but no `payment`, and a client that has
                // written a payment will still echo `payment:<id>@1` at it. There is
                // nothing here to wait for, so stop rather than spin until the
                // timeout.
                Err(_) => return false,
            }
            tokio::time::sleep(POLL).await;
        }
    })
    .await
    .unwrap_or(false)
}

async fn current(db: &PgPool, sql: &str, id: &Uuid) -> Result<Option<u64>, sqlx::Error> {
    let version: Option<i64> = sqlx::query_scalar(sql).bind(id).fetch_optional(db).await?;
    Ok(version.map(|v| v.max(0) as u64))
}

/// `"user:019f…@7,spot:01a0…@3"` -> the entries that parse.
///
/// Malformed entries are skipped rather than rejecting the header: this is an
/// optimisation, not an authorization input, and one junk entry must not cost a
/// client the positions it got right.
fn parse(value: &str) -> impl Iterator<Item = (String, Uuid, u64)> + '_ {
    value.split(',').filter_map(|entry| {
        let (aggregate, version) = parse_version(entry.trim())?;
        let (table, id) = split_aggregate(aggregate)?;
        Some((table.to_string(), id, version))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(header: &str) -> Vec<(String, Uuid, u64)> {
        parse(header).collect()
    }

    #[test]
    fn parses_and_shrugs_off_junk() {
        let id = Uuid::now_v7();
        let good = format!("user:{id}@7");

        assert_eq!(parsed(&good), [("user".to_string(), id, 7)]);

        // Every shape that used to be valid under the old stream-position header,
        // and every shape a client could mangle.
        assert!(parsed("SPOTS:4712").is_empty(), "old stream form must not parse");
        assert!(parsed("user:not-a-uuid@7").is_empty());
        assert!(parsed(&format!("user:{id}@notanumber")).is_empty());
        assert!(parsed(&format!("user:{id}")).is_empty(), "no version");
        assert!(parsed("").is_empty());
    }

    /// One junk entry must not discard the entries around it.
    #[test]
    fn keeps_the_good_entries_beside_a_bad_one() {
        let (a, b) = (Uuid::now_v7(), Uuid::now_v7());
        let header = format!("user:{a}@2,garbage,spot:{b}@5");

        assert_eq!(
            parsed(&header),
            [("user".to_string(), a, 2), ("spot".to_string(), b, 5)]
        );
    }
}
