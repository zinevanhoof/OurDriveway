use chrono::{DateTime, Utc};
use uuid::Uuid;

// No `ViewPayoutPatch`, even though a payout is no longer written once: the only column
// that changes is `status`, and the projector's two arms set it directly rather than
// through a struct with one populated field. `transfer_id` and `failure_reason` are
// deliberately NOT projected — they are Stripe handles and a log message, and nothing a
// browser can reach has any use for either, exactly as with `payment.intent_id`.

/// The `payout` table in the read model — a host's withdrawals.
///
/// One of the two tables PAYMENTS feeds; [`super::payment::ViewPayment`] is the other,
/// and the note there records why that stopped being a rule against. These rows are
/// read as one of the four sources of a wallet month rather than as a list of their
/// own: withdrawals beside the charges they came from is the point.
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
    /// `requested`, `paid` or `failed`, mirrored from payment-service. No enum and no
    /// CHECK: that service's table is the authority, and a status it adds must be a row
    /// this wallet ignores rather than a projector that stops.
    ///
    /// The wallet reads it twice — as the PENDING chip (`requested`) and as the filter
    /// that keeps failed withdrawals out of both the list and the balance.
    pub status: String,
    pub created_at: DateTime<Utc>,
}

// No unit tests: no patch struct to keep in step and no SQL in this file.
//
// `a_payout_upsert_clears_then_relinks_owner` is gone with the thing it tested. It
// existed because a `CONTENT $row` write CLEARED the `owner` record link and
// `link_owner` had to put it back in the same transaction — a two-halves-must-both-run
// sequence that only a database could check. There is no link and no second half now.
