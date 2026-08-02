//! Event vocabulary: the envelope, the subject grammar, and shard assignment.
//!
//! Lives in `shared` (not `bus`) because these are plain serde types over models
//! that already live here, and because keeping them free of `tokio`/`async-nats`
//! means anything can read an event without pulling in a runtime.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub mod booking;
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
    /// The only clock a projector may read.
    pub occurred_at: DateTime<Utc>,
    /// `"user:abc"` — whoever caused this, from a verified JWT claim. `None` for
    /// events raised by a service rather than a request.
    pub actor_id: Option<String>,
    pub payload: T,
}

impl<T> Envelope<T> {
    pub fn new(payload: T, actor_id: Option<String>) -> Self {
        Self {
            event_id: Uuid::now_v7(),
            occurred_at: Utc::now(),
            actor_id,
            payload,
        }
    }
}

// ─── Streams ────────────────────────────────────────────────────────────────
//
// One stream per bounded context. Each binds `<domain>.*.>` so the wildcard
// covers every shard — growing SHARD_COUNT never requires touching a stream.

pub const STREAM_USERS: &str = "USERS";
pub const STREAM_SESSIONS: &str = "SESSIONS";
pub const STREAM_SPOTS: &str = "SPOTS";
pub const STREAM_BOOKINGS: &str = "BOOKINGS";

/// `(stream, subject filter, max_age)`. `max_age` is `None` for streams that are
/// the source of truth and must never expire.
pub const STREAMS: &[(&str, &str, Option<std::time::Duration>)] = &[
    (STREAM_USERS, "users.*.>", None),
    // Sessions are not a source of truth — a refresh token that expired 31 days
    // ago can't be revoked or renewed, so keeping its events forever buys nothing.
    (
        STREAM_SESSIONS,
        "sessions.*.>",
        Some(std::time::Duration::from_secs(31 * 24 * 60 * 60)),
    ),
    (STREAM_SPOTS, "spots.*.>", None),
    (STREAM_BOOKINGS, "bookings.*.>", None),
];

// ─── Sharding ───────────────────────────────────────────────────────────────

/// Number of shards new entities are spread across.
///
/// Raising this is purely additive: a shard is assigned once at entity creation
/// and stored on the record, so existing entities keep publishing to the subject
/// they always did. Nothing is ever rehashed — that would split an entity's
/// history across two subjects and invalidate its CAS sequence.
pub const SHARD_COUNT: u8 = 16;

/// Shard for a freshly minted id. Call once, at creation, then persist the result.
pub fn shard_of(id: &Uuid) -> String {
    // Last byte of a v7 uuid is random; the leading bytes are a timestamp and
    // would bucket every entity created in the same millisecond together.
    format!("{:02}", id.as_bytes()[15] % SHARD_COUNT)
}

// ─── Subjects ───────────────────────────────────────────────────────────────
//
//   <domain>.<shard>.<entity>.<id>
//
// The id is a UUIDv7; its hyphens are legal in a subject token (only `.`, `*`,
// `>` and whitespace are special).

pub fn user_subject(shard: &str, user_id: &Uuid) -> String {
    format!("users.{shard}.user.{}", user::record_key(user_id))
}

pub fn session_subject(shard: &str, user_id: &Uuid) -> String {
    format!("sessions.{shard}.user.{}", user::record_key(user_id))
}

pub fn spot_subject(shard: &str, spot_id: &Uuid) -> String {
    format!("spots.{shard}.spot.{}", user::record_key(spot_id))
}

/// Bookings shard by **spot**, not by booking: it puts every booking for one spot
/// on a single subject, which is what makes a per-spot compare-and-swap possible
/// (`Nats-Expected-Last-Subject-Sequence`) when the booking write path lands.
pub fn booking_subject(spot_shard: &str, spot_id: &Uuid) -> String {
    format!("bookings.{spot_shard}.spot.{}", user::record_key(spot_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shards_are_two_digits_and_in_range() {
        for _ in 0..500 {
            let s = shard_of(&Uuid::now_v7());
            assert_eq!(s.len(), 2, "shard must be zero-padded: {s}");
            assert!(s.parse::<u8>().unwrap() < SHARD_COUNT);
        }
    }

    #[test]
    fn shard_is_stable_for_an_id() {
        let id = Uuid::now_v7();
        assert_eq!(shard_of(&id), shard_of(&id));
    }

    #[test]
    fn subjects_match_their_stream_filter() {
        // A subject must land in exactly the stream that claims it, or events
        // silently vanish into a stream nobody consumes.
        let id = Uuid::now_v7();
        let sh = shard_of(&id);
        for (subject, expected) in [
            (user_subject(&sh, &id), STREAM_USERS),
            (session_subject(&sh, &id), STREAM_SESSIONS),
            (spot_subject(&sh, &id), STREAM_SPOTS),
            (booking_subject(&sh, &id), STREAM_BOOKINGS),
        ] {
            let matched: Vec<_> = STREAMS
                .iter()
                .filter(|(_, filter, _)| {
                    let prefix = filter.trim_end_matches("*.>");
                    subject.starts_with(prefix)
                })
                .map(|(name, _, _)| *name)
                .collect();
            assert_eq!(matched, vec![expected], "subject {subject} matched {matched:?}");
        }
    }
}
