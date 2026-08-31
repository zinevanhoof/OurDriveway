use chrono::{DateTime, Utc};
use uuid::Uuid;

// No `ViewPayoutPatch`: a payout is written once and never edited. Reversing one
// would be another row, not a change to this one.

/// The `payout` table in the read model — a host's withdrawal *history*.
///
/// This is the only thing PAYMENTS contributes to the read model, deliberately. A
/// renter's charges and a host's balance are payment-service's to answer, and
/// projecting them here too would give the payout button one number and the balance
/// beside it another. What belongs here is the list, so it can be queried alongside
/// the rest of a profile like everything else.
///
/// There is no `owner` link column any more, and nothing to relink: the read model
/// holds plain uuids and resolves references with a LEFT JOIN at read time.
#[derive(Clone, Debug, sqlx::FromRow)]
pub struct ViewPayout {
    pub id: Uuid,
    /// The owning service's version of this aggregate. What `bus::await_version`
    /// compares a client's `X-Await-Version` against.
    #[sqlx(try_from = "i64")]
    pub version: u64,
    pub owner_id: Uuid,
    /// EUR cents.
    pub amount: i64,
    pub created_at: DateTime<Utc>,
}

// No unit tests: no patch struct to keep in step and no SQL in this file.
//
// `a_payout_upsert_clears_then_relinks_owner` is gone with the thing it tested. It
// existed because a `CONTENT $row` write CLEARED the `owner` record link and
// `link_owner` had to put it back in the same transaction — a two-halves-must-both-run
// sequence that only a database could check. There is no link and no second half now.
