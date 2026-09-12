//! Event vocabulary: the envelope and the subject grammar.
//!
//! Lives in `shared` (not `bus`) because these are plain serde types over models
//! that already live here, and because keeping them free of `tokio`/`async-nats`
//! means anything can read an event without pulling in a runtime.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub mod booking;
pub mod payment;
pub mod session;
pub mod spot;
pub mod user;

/// Wraps every event on every stream.
///
/// Everything non-deterministic lives here rather than being computed by a
/// projector: replicas replay the same events independently, so a clock read or
/// a generated id inside a projection would make them disagree forever.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Envelope<T> {
    /// UUIDv7. Doubles as the `Nats-Msg-Id` dedupe key.
    pub event_id: Uuid,
    /// Which aggregate this event is about, as `<table>:<uuid>` — `user:019f…`.
    ///
    /// The same string a client waits on, and the key a consumer stores
    /// [`Self::version`] against. A qualified name rather than a bare uuid because
    /// consumers hold several tables and `booking:<id>` and `payment:<id>` are
    /// different rows.
    pub aggregate: String,
    /// This aggregate's version **after** the event was applied by its host.
    ///
    /// Assigned inside the writing transaction, so it exists the moment the write
    /// commits — which is the whole reason it replaced the NATS stream sequence.
    /// The sequence is only known once the relay has published, long after the
    /// route has to answer.
    ///
    /// Monotonic per aggregate and gapless: a consumer holding 3 that receives 5
    /// knows it is missing 4, rather than applying it out of order.
    ///
    /// `i64`, matching the `bigint` column every version is read from and written to.
    /// It was a `u64` on the grounds that a negative version should not be
    /// representable, which cost a newtype at the column boundary and three
    /// hand-written clamps at the `sql_query` reads that boundary never sees. JSON has
    /// no signedness, so events written before that change decode after it.
    pub version: i64,
    /// The only clock a projector may read.
    pub occurred_at: DateTime<Utc>,
    /// Whoever caused this, from a verified JWT claim. `None` for events raised by
    /// a service rather than a request.
    pub actor_id: Option<Uuid>,
    /// True when this is a re-emission of current state rather than something that
    /// just happened — see `bus::outbox::backfill`.
    ///
    /// **Projectors must ignore this.** Rebuilding a projection is the whole point,
    /// and a projector that skipped these would rebuild nothing. It exists for
    /// consumers whose reaction is a side effect *outside* this system: a backfill
    /// of USERS re-emits `Registered` for every account, and notification-service
    /// would send every one of them a fresh verification email.
    ///
    /// `#[serde(default)]` so an event encoded before this field existed still
    /// decodes — and decodes as `false`, which is the safe direction.
    #[serde(default)]
    pub backfill: bool,
    pub payload: T,
}

impl<T> Envelope<T> {
    /// `aggregate` is `<table>:<uuid>` — build it with [`aggregate_id`].
    pub fn new(payload: T, actor_id: Option<Uuid>, aggregate: String, version: i64) -> Self {
        Self {
            event_id: Uuid::now_v7(),
            aggregate,
            version,
            occurred_at: Utc::now(),
            actor_id,
            backfill: false,
            payload,
        }
    }
}

/// `("user", id)` -> `"user:019f…"`. The one place this string is shaped.
///
/// Matches SurrealDB's own record-id spelling on purpose: a consumer storing a
/// version against it is usually storing it on that very row.
pub fn aggregate_id(table: &str, id: &Uuid) -> String {
    format!("{table}:{id}")
}

/// What a route hands back for the client to echo — `"user:019f…@7"`.
///
/// Replaces `format_seq`, which named a stream position. A client that waited on
/// `SPOTS:4712` waited for every spot; this waits for one aggregate to reach one
/// version, which is both narrower and answerable at commit time.
pub fn format_version(aggregate: &str, version: i64) -> String {
    format!("{aggregate}@{version}")
}

/// Splits `"user:019f…@7"` back into its halves. The inverse of
/// [`format_version`], kept beside it so the two cannot drift.
///
/// **Parsed as `u64` and widened**, which is the one place unsignedness was ever load
/// bearing: this reads a header a client writes, and `"-5".parse::<u64>()` fails, so
/// `user:<id>@-5` is dropped here rather than becoming a wait that is satisfied before
/// it starts. Everywhere else a version is an `i64`, because everywhere else it comes
/// from `next_version` or from the column it was written to.
pub fn parse_version(s: &str) -> Option<(&str, i64)> {
    let (aggregate, version) = s.rsplit_once('@')?;
    Some((aggregate, version.parse::<u64>().ok()?.try_into().ok()?))
}

/// `"user:019f…"` -> `("user", <uuid>)`. The inverse of [`aggregate_id`].
///
/// `split_once`, not `rsplit_once`: a table name never contains `:` and a uuid
/// never does either, but splitting from the left is what keeps a future
/// namespaced table (`app:user:<id>`) parsing as its table rather than its id.
pub fn split_aggregate(aggregate: &str) -> Option<(&str, Uuid)> {
    let (table, id) = aggregate.split_once(':')?;
    Some((table, id.parse().ok()?))
}

// ─── Streams ────────────────────────────────────────────────────────────────
//
// One stream per bounded context, each binding `<domain>.>`.
//
// The filter used to be `<domain>.*.>`, where the `*` matched a shard token that
// every subject carried and `shard_of(id)` assigned in Rust. That token is gone;
// what replaced it is [`PARTITIONS`], which is the same idea moved to where it
// belongs — NATS assigns it on ingest, the application never computes it, and it
// exists to parallelise *consumption* rather than to place rows.

pub const STREAM_USERS: &str = "USERS";
pub const STREAM_SESSIONS: &str = "SESSIONS";
pub const STREAM_SPOTS: &str = "SPOTS";
pub const STREAM_BOOKINGS: &str = "BOOKINGS";
pub const STREAM_PAYMENTS: &str = "PAYMENTS";

const DAY: u64 = 24 * 60 * 60;

/// How many ordered lanes each stream is cut into.
///
/// NATS hashes every subject into one of these on ingest (see
/// [`partition_transform`]) and a projector runs one consumer per partition, each
/// with `max_ack_pending: 1`. Same subject → same partition → strict order;
/// different partitions run concurrently.
///
/// **A `const`, deliberately not an environment variable.** `bus::ensure_streams`
/// runs in every service at boot and declares the same streams. If two services
/// disagreed about this number, the first to boot would win and the other's lane
/// filters would match nothing — for ever, silently. A const makes disagreement
/// impossible and makes a change a visible commit.
///
/// Changing it is not a hot edit: a key moves lanes while its old lane may still
/// hold unconsumed events for it, so the streams must be drained first. See the
/// migration note in `bus::connect::ensure_streams`.
///
/// 16 rather than 64: this is the ceiling on concurrent applies per stream *no
/// matter how many replicas run*, and each lane costs one JetStream consumer per
/// projector plus one SurrealDB session. Raise it when the projectors are measurably
/// the bottleneck, not before.
pub const PARTITIONS: u8 = 16;

/// `(stream, domain, max_age)`.
///
/// `domain` is the first subject token, not the whole filter. The filter
/// (`users.>`), the transform source (`users.*.*`) and a lane's filter
/// (`users.7.>`) are all derived from it, so they cannot drift apart.
///
/// **Every stream expires.** Four of these used to be `None` — infinite — because
/// each service database was an `emptyDir` wiped on every pod restart and a
/// projection could only be rebuilt by replaying from sequence 1. That was normal
/// operation, several times a day, so the log genuinely was the source of truth.
///
/// TiKV is now, and it survives restarts. What is left is an integration bus: a
/// week is far longer than any consumer is ever behind, and a projection that does
/// need re-deriving is re-emitted from current state by each owning service —
/// `bus::outbox::backfill`, reached at `POST /internal/backfill`.
///
/// One thing that genuinely goes away with the old retention: PAYMENTS stopped
/// being an audit trail of every charge and refund. That record is the `payment`
/// and `payout` tables now. If an *independent* ledger is ever wanted it has to be
/// an explicit table somewhere else, not a side effect of never deleting anything.
pub const STREAMS: &[(&str, &str, std::time::Duration)] = &[
    (
        STREAM_USERS,
        "users",
        std::time::Duration::from_secs(7 * DAY),
    ),
    // The one that was already bounded, and the longest: a refresh token lives 31
    // days, so its events stop meaning anything at exactly that point.
    (
        STREAM_SESSIONS,
        "sessions",
        std::time::Duration::from_secs(31 * DAY),
    ),
    (
        STREAM_SPOTS,
        "spots",
        std::time::Duration::from_secs(7 * DAY),
    ),
    (
        STREAM_BOOKINGS,
        "bookings",
        std::time::Duration::from_secs(7 * DAY),
    ),
    (
        STREAM_PAYMENTS,
        "payments",
        std::time::Duration::from_secs(7 * DAY),
    ),
];

/// What a stream binds — every subject in its bounded context, partitioned or not.
///
/// Stays `<domain>.>` rather than `<domain>.*.*`: the transform rewrites subjects
/// *after* they match this, so narrowing it would reject the very messages it is
/// meant to partition.
pub fn stream_filter(domain: &str) -> String {
    format!("{domain}.>")
}

/// `"USERS"` -> `"users"`. The one place a stream name becomes a subject token.
pub fn domain_of(stream: &str) -> Option<&'static str> {
    STREAMS
        .iter()
        .find(|(name, ..)| *name == stream)
        .map(|(_, domain, _)| *domain)
}

/// The stream subject transform that assigns a partition on ingest:
/// `("users.*.*", "users.{{partition(16,1,2)}}.{{wildcard(1)}}.{{wildcard(2)}}")`.
///
/// NATS applies this as it stores each message, so a publisher keeps sending the
/// three-token subject and consumers see the four-token one. Nothing in this
/// codebase hashes anything.
///
/// **Hashed on both wildcards**, because the ordering key is the whole
/// `<entity>.<id>` pair — `payments.booking.<id>` and `payments.payout.<id>` are
/// different aggregates on one stream and have no reason to share a lane.
///
/// The source has three tokens and the destination four, so a stored subject can
/// never match the source again — the transform cannot compound, whatever else is
/// later done to these streams.
///
/// Only `<domain>.<entity>.<id>` is transformed. A subject of any other shape still
/// matches [`stream_filter`] and would be stored untransformed, where no lane filter
/// reaches it — which is why `subjects_are_three_tokens` guards the grammar.
pub fn partition_transform(domain: &str) -> (String, String) {
    (
        format!("{domain}.*.*"),
        format!(
            "{domain}.{{{{partition({PARTITIONS},1,2)}}}}.{{{{wildcard(1)}}}}.{{{{wildcard(2)}}}}"
        ),
    )
}

/// What one lane's consumer filters on — `"users.7.>"`.
pub fn partition_filter(domain: &str, partition: u8) -> String {
    format!("{domain}.{partition}.>")
}

// ─── Subjects ───────────────────────────────────────────────────────────────
//
//   <domain>.<entity>.<id>
//
// The id is a UUIDv7; its hyphens are legal in a subject token (only `.`, `*`,
// `>` and whitespace are special).
//
// There used to be a `<shard>` token between domain and entity, assigned by
// `shard_of(id)` from the last byte of the uuid, stored on every record and
// echoed on every event. It existed so a future deployment could route one
// service instance per shard. TiKV partitions its own keyspace into regions and
// splits them as they grow, so nothing in the application needs to decide where
// a row lives — and a shard token the application no longer reads is a field on
// every table and event that can only drift.
//
// What is NOT gone is the choice of KEY each subject is built from. Those are
// unchanged and still load-bearing, because they decide what shares an ordering:
// bookings key on their spot, payments on their booking, payouts on their host.
//
// The three-token shape is now load-bearing too. `partition_transform` matches
// `<domain>.*.*` exactly, so a subject with any other token count is stored
// untransformed and no lane filter reaches it — it would sit in the stream
// unconsumed rather than fail loudly. `subjects_are_three_tokens` is what stops
// that being discovered in production.

pub fn user_subject(user_id: &Uuid) -> String {
    format!("users.user.{user_id}")
}

pub fn session_subject(user_id: &Uuid) -> String {
    format!("sessions.user.{user_id}")
}

pub fn spot_subject(spot_id: &Uuid) -> String {
    format!("spots.spot.{spot_id}")
}

/// Bookings are keyed by **spot**, not by booking: it puts every booking for one
/// spot on a single subject, which is what gives them a total order.
///
/// That ordering is what prevents a double booking. It backed the per-spot
/// compare-and-swap (`Nats-Expected-Last-Subject-Sequence`) and, once TiKV is
/// authoritative, it is the per-spot version bump that replaces it — either way the
/// grain is the spot.
pub fn booking_subject(spot_id: &Uuid) -> String {
    format!("bookings.spot.{spot_id}")
}

/// Payments are keyed by **booking**, unlike bookings which key on spot.
///
/// The spot-keyed subject exists to give every booking on one spot a total order,
/// because that ordering is what prevents a double booking. Payments need no such
/// invariant — a payment concerns exactly one booking and races nothing — so the
/// booking is the natural grain, and it keeps one booking's payment history
/// (created, succeeded, refunded) on a single subject where it can be read back in
/// order.
pub fn payment_subject(booking_id: &Uuid) -> String {
    format!("payments.booking.{booking_id}")
}

/// A host withdrawing their balance, on the same stream but a different entity.
///
/// The grammar is `<domain>.<entity>.<id>`, so a payout is not forced onto a
/// booking-keyed subject it has nothing to do with — it concerns a host and an amount,
/// no booking at all. Both subjects match `payments.>` and land in the same stream,
/// which is what keeps one projector able to see the whole money history in order.
///
/// Keyed by **host**, not by payout, and for the same reason bookings are keyed by
/// spot: it puts every one of a host's withdrawals on a single subject, which is what
/// serializes them. Two withdraw requests racing — a double-clicked button — both read
/// the same balance and both look affordable; that ordering is what makes exactly one
/// of them win instead of paying out twice.
pub fn payout_subject(host_id: &Uuid) -> String {
    format!("payments.payout.{host_id}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every event subject, and the stream each must land in.
    fn every_subject(id: &Uuid) -> [(String, &'static str); 6] {
        [
            (user_subject(id), STREAM_USERS),
            (session_subject(id), STREAM_SESSIONS),
            (spot_subject(id), STREAM_SPOTS),
            (booking_subject(id), STREAM_BOOKINGS),
            (payment_subject(id), STREAM_PAYMENTS),
            (payout_subject(id), STREAM_PAYMENTS),
        ]
    }

    /// Which streams claim a subject. `<domain>.>` matches on the first token.
    fn streams_claiming(subject: &str) -> Vec<&'static str> {
        STREAMS
            .iter()
            .filter(|(_, domain, _)| {
                subject
                    .split_once('.')
                    .is_some_and(|(first, _)| first == *domain)
            })
            .map(|(name, ..)| *name)
            .collect()
    }

    #[test]
    fn subjects_match_their_stream_filter() {
        // A subject must land in exactly the stream that claims it, or events
        // silently vanish into a stream nobody consumes. This is what catches a
        // subject helper and a STREAMS domain drifting apart.
        let id = Uuid::now_v7();
        for (subject, expected) in every_subject(&id) {
            let matched = streams_claiming(&subject);
            assert_eq!(
                matched,
                vec![expected],
                "subject {subject} matched {matched:?}"
            );
        }
    }

    /// A subject that lands in a stream but is not `<domain>.<entity>.<id>` is
    /// stored untransformed, and no lane filter reaches it — it accrues in the
    /// stream silently instead of erroring.
    ///
    /// `SUBJECT_SPOT_CARD` was exactly that: `spots.card` matched `spots.>`, so
    /// every RPC request was stored in the SPOTS stream, and the unfiltered
    /// projectors then tried to decode one as an `Envelope<SpotEvent>` and stopped.
    /// It lives under `rpc.` now; this is what keeps it there.
    #[test]
    fn only_event_subjects_land_in_streams() {
        assert!(
            streams_claiming(crate::rpc::spot::SUBJECT_SPOT_CARD).is_empty(),
            "an RPC subject must not be captured by an event stream"
        );
    }

    /// The transform only rewrites what it matches, so every subject a service
    /// publishes has to match its source pattern.
    #[test]
    fn every_subject_is_partitioned() {
        let id = Uuid::now_v7();
        for (subject, stream) in every_subject(&id) {
            let domain = domain_of(stream).expect("stream is in STREAMS");
            let (source, destination) = partition_transform(domain);

            // `<domain>.*.*` — same leading token, exactly three tokens.
            let source_tokens: Vec<_> = source.split('.').collect();
            let subject_tokens: Vec<_> = subject.split('.').collect();
            assert_eq!(
                subject_tokens.len(),
                source_tokens.len(),
                "{subject} does not match transform source {source}, so it would be \
                 stored unpartitioned and no lane would consume it"
            );
            assert_eq!(subject_tokens[0], source_tokens[0]);

            // The destination puts the partition where nothing can re-match the
            // source: four tokens against the source's three.
            assert_eq!(destination.split('.').count(), 4, "{destination}");
            assert!(
                destination.contains(&format!("partition({PARTITIONS},1,2)")),
                "{destination} must hash both key tokens"
            );
        }
    }

    /// A lane filter must claim its own partition and nothing else.
    #[test]
    fn lane_filters_do_not_overlap() {
        let filters: Vec<_> = (0..PARTITIONS)
            .map(|p| partition_filter("users", p))
            .collect();

        assert_eq!(filters[0], "users.0.>");
        assert_eq!(filters[7], "users.7.>");

        // `users.1.>` must not also claim `users.11.…`, which is what a naive
        // `starts_with("users.1")` would do. The trailing `.` is doing that work.
        let stored = "users.11.user.019f";
        let claiming: Vec<_> = filters
            .iter()
            .filter(|f| stored.starts_with(f.trim_end_matches('>')))
            .collect();
        assert_eq!(claiming.len(), 1, "{stored} matched {claiming:?}");
        assert_eq!(claiming[0], "users.11.>");
    }

    #[test]
    fn a_version_round_trips() {
        let id = Uuid::now_v7();
        let agg = aggregate_id("user", &id);
        let version = format_version(&agg, 7);

        assert_eq!(version, format!("user:{id}@7"));
        assert_eq!(parse_version(&version), Some((agg.as_str(), 7)));
    }

    /// `rsplit_once`, not `split_once`: the aggregate half contains a uuid, and a
    /// uuid contains no `@` — but splitting from the right is what keeps this
    /// correct if an aggregate name ever does.
    #[test]
    fn parsing_takes_the_last_at_sign() {
        assert_eq!(parse_version("a@b:c@12"), Some(("a@b:c", 12)));
        assert_eq!(parse_version("no-version-here"), None);
        assert_eq!(parse_version("user:x@notanumber"), None);
    }

    /// The grammar is `<domain>.<entity>.<id>` — three tokens, and the id last.
    ///
    /// Both halves are load-bearing for the partition transform, not just tidy:
    /// `partition_transform` matches exactly three tokens and hashes tokens 1 and 2.
    /// A stray token would still match the stream's `<domain>.>` filter, so the
    /// tests above would pass while the message was stored unpartitioned and no
    /// lane consumed it.
    #[test]
    fn subjects_are_three_tokens() {
        let id = Uuid::now_v7();
        for (subject, _) in every_subject(&id) {
            let tokens: Vec<_> = subject.split('.').collect();
            assert_eq!(
                tokens.len(),
                3,
                "expected <domain>.<entity>.<id>: {subject}"
            );
            assert_eq!(tokens[2], id.to_string(), "id must be the last token");
        }
    }
}
