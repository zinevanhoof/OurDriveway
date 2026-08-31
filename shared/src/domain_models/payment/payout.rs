use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::events::payment::PaymentEvent;

/// A host withdrawing their balance.
///
/// **Nothing real moves** — no Stripe Connect account, no transfer, no bank. This is
/// the demo of a payout, and the row exists only so the balance goes down and stays
/// down across a reload.
///
/// There is deliberately no stored balance to decrement: available is always
/// `earnings − Σ payout.amount_cents`, computed on read. That is what makes a
/// refunded booking drop out of a host's income for free rather than needing a
/// compensating write here.
#[derive(Clone, Debug, sqlx::FromRow)]
pub struct Payout {
    pub id: Uuid,
    /// Written once and never bumped — a payout does not change — but still read,
    /// because a backfill re-emitting `PayoutRequested` has to stamp the version
    /// view-service already recorded against it.
    #[sqlx(try_from = "i64")]
    pub version: u64,
    pub owner_id: Uuid,
    pub amount_cents: i64,
    pub created_at: DateTime<Utc>,
}

impl Payout {
    // There is deliberately no `PayoutPatch`: a payout is written once and never
    // changes. Reversing one would be another row, not an edit to this one.

    /// The row a `PayoutRequested` writes, or `None` for any other payment event.
    ///
    /// Takes the whole event rather than the four fields so the projector's arm
    /// stays a single line, and so the mapping lives next to the row it produces.
    pub fn requested(event: &PaymentEvent, version: u64) -> Option<Self> {
        match event {
            PaymentEvent::PayoutRequested {
                payout_id,
                owner_id,
                amount_cents,
                requested_at,
            } => Some(Self {
                id: *payout_id,
                version,
                owner_id: *owner_id,
                amount_cents: *amount_cents,
                // The requester's timestamp off the event, not this replica's clock.
                created_at: *requested_at,
            }),
            _ => None,
        }
    }
}

// No unit tests: there is no patch struct to keep in step and no SQL in this file.
// A new column does need an edit in `PayoutRepository::upsert` now — sqlx has no
// whole-struct write to match SurrealDB's `CONTENT $row`, so the column list is
// spelled out there. `payouts_sum_per_owner` in payment-service covers the round-trip.
