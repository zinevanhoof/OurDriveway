use surrealdb::types::{Datetime, SurrealValue};
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
/// `owner` — the `record<user>` link — is not on this model; see the module doc.
#[derive(Clone, Debug, SurrealValue)]
pub struct ViewPayout {
    pub id: Uuid,
    /// The owning service's version of this aggregate, carried so a `CONTENT $row`
    /// write does not clear the column — see [`super::user::ViewUser::version`] for
    /// what happens when it does.
    pub version: u64,
    pub owner_id: Uuid,
    /// EUR cents.
    pub amount: i64,
    pub created_at: Datetime,
}

// No unit tests: no patch struct to keep in step and no SQL in this file. That the
// `owner` link is not a field here — so a `CONTENT $row` write clears it and
// `ViewPayoutRepository::link_owner` puts it back in the same transaction — is
// checked against a real database by `a_payout_upsert_clears_then_relinks_owner`.
