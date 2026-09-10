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
// Re-exported so `version_reader!` can name them without every service having to depend
// on `futures` or import `shared::db::Db` for a signature the macro wrote.
pub use futures::future::BoxFuture;
pub use shared::db::Db;
use shared::events::{parse_version, split_aggregate};
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

/// What a service can answer about one aggregate, having looked in its own tables.
///
/// Three outcomes and not `Option<i64>`, because "no row yet" and "not stored here" want
/// opposite reactions: the first is the ordinary case for a read that follows a create
/// closely and must keep waiting, the second means there is nothing here to wait for and
/// spinning until the timeout would be a two-second pause for nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Applied {
    /// The row is there, at this version.
    At(i64),
    /// The aggregate is stored here but this row has not been written yet. Keep waiting.
    Pending,
    /// Nothing to wait for: this database does not store this aggregate, or it declined
    /// to answer. Stop rather than spin.
    Unavailable,
}

/// How `bus` asks a service for a version without knowing the service's tables.
///
/// The aggregate name arrives from a client's `X-Await-Version` header, so it is a genuine
/// runtime value — but `bus` is linked into all five services and each answers against its
/// own schema module, where `spot` is a different Rust type in each. So the match lives in
/// the service and `bus` holds a pointer to it.
///
/// This replaced a `format!`-spliced `SELECT version FROM {table}` guarded by
/// `shared::db::table_for`'s allowlist. Write one with [`version_reader!`].
pub type VersionReader = for<'a> fn(&'a Db, &'a str, &'a Uuid) -> BoxFuture<'a, Applied>;

/// The database this service serves reads from — the same one its projectors
/// write — and the service's own answer to "what version is this aggregate at?".
#[derive(Clone)]
pub struct AwaitVersions(pub Db, pub VersionReader);

/// Writes a service's [`VersionReader`]: one arm per aggregate it stores.
///
/// Each arm is a real diesel table from that service's own schema module, so the query is
/// built by the DSL and the version comes back as a plain `i64` — no spliced table name,
/// no allowlist, and no `QueryableByName` struct to carry a column called `version`.
///
/// An aggregate with no arm is [`Applied::Unavailable`], which is what a client echoing
/// `booking:<id>@N` at every service gets from the ones that do not store bookings. That
/// used to be a 42P01 from the database; it is an absent match arm now.
///
/// ```ignore
/// bus::version_reader! {
///     pub fn version_of;
///     "spot" => shared::schema::spot::spot,
/// }
/// ```
#[macro_export]
macro_rules! version_reader {
    (
        $(#[$meta:meta])*
        $vis:vis fn $name:ident;
        $($aggregate:literal => $($table:ident)::+),+ $(,)?
    ) => {
        $(#[$meta])*
        $vis fn $name<'a>(
            db: &'a $crate::await_version::Db,
            aggregate: &'a str,
            id: &'a ::uuid::Uuid,
        ) -> $crate::await_version::BoxFuture<'a, $crate::await_version::Applied> {
            use ::diesel::OptionalExtension as _;
            use ::diesel::QueryDsl as _;
            use ::diesel_async::RunQueryDsl as _;
            use $crate::await_version::Applied;

            ::std::boxed::Box::pin(async move {
                let Ok(mut conn) = db.get().await else {
                    // A pool error is not "behind", it is "no answer right now" — and the
                    // caller's only two choices are wait or give up. Giving up matches
                    // what a missing table does.
                    return Applied::Unavailable;
                };

                match aggregate {
                    $(
                        $aggregate => match $($table)::+::table
                            .find(id)
                            .select($($table)::+::version)
                            .first::<i64>(&mut *conn)
                            .await
                            .optional()
                        {
                            Ok(Some(version)) => Applied::At(version),
                            Ok(None) => Applied::Pending,
                            Err(_) => Applied::Unavailable,
                        },
                    )+
                    _ => Applied::Unavailable,
                }
            })
        }
    };
}

pub async fn await_version(
    axum::extract::State(db): axum::extract::State<AwaitVersions>,
    request: Request,
    next: Next,
) -> Response {
    if let Some(header) = request.headers().get(AWAIT_VERSION) {
        let wanted: Vec<_> = parse(header.to_str().unwrap_or_default()).collect();

        let _ = tokio::time::timeout(TIMEOUT, async {
            for (table, id, want) in wanted {
                wait_one(&db.0, db.1, &table, &id, want).await;
            }
        })
        .await;
    }
    next.run(request).await
}

/// Blocks until `aggregate:id` has reached `want`, or the caller's timeout fires.
async fn wait_one(db: &Db, read: VersionReader, table: &str, id: &Uuid, want: i64) {
    // The outer [`TIMEOUT`] in the middleware caps the whole header, so this one is
    // only a backstop for a single entry.
    let _ = reached(db, read, table, id, want, TIMEOUT).await;
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
    db: &Db,
    read: VersionReader,
    aggregate: &str,
    id: &Uuid,
    want: i64,
    timeout: Duration,
) -> bool {
    tokio::time::timeout(timeout, async {
        loop {
            match read(db, aggregate, id).await {
                Applied::At(have) if have >= want => return true,
                // Behind, or the row is not there yet — its creating event has not been
                // applied. Keep waiting; this is the common case for a read that follows
                // a create closely.
                Applied::At(_) | Applied::Pending => {}
                // This database does not store this aggregate, which is not a fault:
                // every service answers this header against its own schema, and a
                // client that has written a booking echoes `booking:<id>@N` at all of
                // them. There is nothing here to wait for, so stop rather than spin
                // until the timeout.
                //
                // This used to be a 42P01 escaping from a spliced query. It is an absent
                // match arm in the service's `version_reader!` now, which is the same
                // answer decided at compile time.
                //
                // `payment` used to be the example — view-service held `payout` and no
                // `payment`, so echoing a payment's version at it returned immediately.
                // It holds both now, which means that wait is real: a client that has
                // just paid, or just withdrawn, waits for the projector rather than
                // reading a wallet without the thing it did in it.
                Applied::Unavailable => return false,
            }
            tokio::time::sleep(POLL).await;
        }
    })
    .await
    .unwrap_or(false)
}

// `current` was here: one poll, through `sql_query` on a `format!`-spliced table name,
// reading back a `QueryableByName` struct whose only field was `version: i64`.
//
// Each service's [`version_reader!`] does that now, against real tables from its own
// schema module — so the spliced name is gone, `shared::db::table_for`'s allowlist has no
// caller left to guard, and `.select(version)` loads straight into `i64` with no struct.
//
// Its two error arms survive as [`Applied::Unavailable`]: a pool error and a query error
// were folded together on purpose, because both mean "no answer right now" and the
// caller's only two choices are wait or give up.
//
// No clamp on the value, then or now: the column and the answer are the same signed type,
// and a stored version below the one being waited for is what "not there yet" means.

/// `"user:019f…@7,spot:01a0…@3"` -> the entries that parse.
///
/// Malformed entries are skipped rather than rejecting the header: this is an
/// optimisation, not an authorization input, and one junk entry must not cost a
/// client the positions it got right.
fn parse(value: &str) -> impl Iterator<Item = (String, Uuid, i64)> + '_ {
    value.split(',').filter_map(|entry| {
        let (aggregate, version) = parse_version(entry.trim())?;
        let (table, id) = split_aggregate(aggregate)?;
        Some((table.to_string(), id, version))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(header: &str) -> Vec<(String, Uuid, i64)> {
        parse(header).collect()
    }

    #[test]
    fn parses_and_shrugs_off_junk() {
        let id = Uuid::now_v7();
        let good = format!("user:{id}@7");

        assert_eq!(parsed(&good), [("user".to_string(), id, 7)]);

        // Every shape that used to be valid under the old stream-position header,
        // and every shape a client could mangle.
        assert!(
            parsed("SPOTS:4712").is_empty(),
            "old stream form must not parse"
        );
        assert!(parsed("user:not-a-uuid@7").is_empty());
        assert!(parsed(&format!("user:{id}@notanumber")).is_empty());
        assert!(parsed(&format!("user:{id}")).is_empty(), "no version");
        assert!(parsed("").is_empty());

        // A version is an `i64` everywhere now, so this is the one place that still has
        // to refuse a negative — `parse_version` parses as `u64` and widens for exactly
        // this. A `-5` that got through would be a wait already satisfied, which is a
        // read served before the write the client is echoing.
        assert!(
            parsed(&format!("user:{id}@-5")).is_empty(),
            "a negative version must not parse"
        );
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
