use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

/// `GET /api/view/me/payouts` — one row of the caller's withdrawal history.
///
/// Payments themselves are deliberately absent from the read model. What a renter was
/// charged and what a host has available are payment-service's to answer, and a second
/// copy here would eventually disagree with the balance shown next to the withdraw
/// button.
///
/// The three columns and the three response fields are genuinely the same three things,
/// so there is no aliasing and nothing to nest.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct PayoutListItem {
    pub id: Uuid,
    /// EUR cents.
    pub amount: i64,
    pub created_at: DateTime<Utc>,
}
