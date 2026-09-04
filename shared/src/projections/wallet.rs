use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::general_models::booking::Booked;

/// What a wallet row *is*, as stored in `kind`.
///
/// The SQL spells these as literals in each `UNION ALL` branch — a const cannot be
/// spliced into a statement without `format!`, which the repository layer does not do.
/// They are written once here so the policy that folds them, and the tests over it,
/// cannot drift from the four branches by a typo.
///
/// The kind is the **icon**; the sign of `amount_cents` is the direction. They are not
/// redundant: a refund is money back to a renter and money away from a host, and both
/// are `refund`.
pub mod kind {
    /// A booking on the caller's own spot, charged. Positive.
    pub const IN: &str = "in";
    /// A booking the caller rented, charged. Negative.
    pub const OUT: &str = "out";
    /// A charge given back. Positive for the renter, negative for the host.
    pub const REFUND: &str = "refund";
    /// The caller withdrawing their own balance. Negative, and deliberately not
    /// counted as money out — see [`crate::projections::wallet::WalletMonth`].
    pub const PAYOUT: &str = "payout";
}

/// One line of the wallet: a single movement of money involving the caller.
///
/// Row *and* response, like every other projection here — `query_as` decodes it and
/// `Json` serialises it with nothing in between.
///
/// **It carries data, not sentences.** `title` is the spot's own title and `booked` is
/// its slots in the spot's wall clock; the client composes the line it shows from those
/// with the same helpers every other booking on screen uses (`lib/bookingDates.ts`).
/// Formatting a date server-side would print it in the *server's* idea of a locale, and
/// building an English label in SQL would put copy somewhere no designer will find it.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct WalletTransaction {
    /// `<uuid>:<kind>`, not the bare payment id.
    ///
    /// One payment can produce **two** rows — the charge and, later, the refund of it —
    /// and a self-booking would produce an `in` and an `out` from the same row. The
    /// suffix is what keeps them distinguishable as list keys.
    pub id: String,
    /// One of [`kind`].
    pub kind: String,
    /// Signed EUR cents, from the caller's point of view: what their balance did.
    pub amount_cents: i64,
    /// When it happened — the charge's session, the refund's return, the withdrawal's
    /// request. This is the field the month grouping and the ordering are on.
    pub occurred_at: DateTime<Utc>,
    /// Host income that has not settled yet: the booking is confirmed but has not been
    /// over for `SETTLEMENT_SECS`, so it is counted in the wallet and not yet
    /// withdrawable. Always false for the other three kinds.
    pub pending: bool,
    /// The spot's title. `None` on a payout, which is about no spot, and on a charge
    /// whose spot has not been projected here yet.
    pub title: Option<String>,
    /// The slots the booking holds, in the spot's own wall clock. `None` on a payout.
    #[sqlx(json(nullable))]
    pub booked: Option<Booked>,
    /// The spot's IANA zone, which is what `booked` is written in. `None` wherever
    /// `booked` is.
    pub timezone: Option<String>,
}

/// One month of a wallet, which is also one page of it.
///
/// The client asks for a month and is told which month to ask for next, so it never
/// walks backwards through months that hold nothing and it knows where the history
/// ends. Rows within a month are not paginated — see the ceiling on
/// `WalletRepository::find_month`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WalletMonth {
    /// `"YYYY-MM"`, as asked for.
    pub month: String,
    /// Everything that came in, as a positive figure.
    pub in_cents: i64,
    /// Everything that went out, as a **positive** figure, and deliberately not
    /// including payouts: moving your own money to your own account is not spending it.
    pub out_cents: i64,
    /// The next older month that holds anything, or `None` at the end of the history.
    pub next_month: Option<String>,
    /// Newest first.
    pub transactions: Vec<WalletTransaction>,
}

/// A host's money, in one object.
///
/// Moved here from payment-service's `EarningsResponse` when reads moved to
/// view-service. `pending_cents` is the one figure that is new: it is what
/// `available_cents` will grow by once the bookings behind it have been over long
/// enough, and it is what the wallet's "clears 24h after each booking ends" line
/// shows.
///
/// Derived on every read, never stored — which is what makes a refunded booking drop
/// out of a host's income with no compensating write anywhere.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Balance {
    /// Withdrawable now: settled income minus what has already been taken out.
    pub available_cents: i64,
    /// Everything earned and settled, ever.
    pub earned_cents: i64,
    pub paid_out_cents: i64,
    /// Earned but not yet settled.
    pub pending_cents: i64,
}
