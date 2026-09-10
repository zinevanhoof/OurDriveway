use chrono::{DateTime, Utc};
use diesel::prelude::*;

use crate::general_models::booking::Booked;

/// What a wallet row *is*, as stored in `kind`.
///
/// The SQL spells these as literals in each `UNION ALL` branch — a const cannot be
/// spliced into a statement without `format!`, which the repository layer does not do.
/// They are written once here so the policy that folds them, and the tests over it,
/// cannot drift from the five branches by a typo.
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
    /// The caller withdrawing their own balance. Negative, and deliberately not counted
    /// as money out — see `responses::view::WalletMonthResponse`.
    pub const PAYOUT: &str = "payout";
}

/// One line of the wallet: a single movement of money involving the caller.
///
/// **It carries data, not sentences.** `title` is the spot's own title and `booked` is
/// its slots in the spot's wall clock; the client composes the line it shows from those
/// with the same helpers every other booking on screen uses (`lib/bookingDates.ts`).
/// Formatting a date server-side would print it in the *server's* idea of a locale, and
/// building an English label in SQL would put copy somewhere no designer will find it.
///
/// `Queryable` and deliberately NOT `Selectable`: this is not a group of columns from a
/// table, it is the shape of a five-branch `UNION ALL` with computed columns, so there is
/// nothing for `as_select()` to mean and no `table_name` to infer from.
///
/// **Positional.** The field order below IS the select-clause order in
/// `WalletRepository::find_month_for_account`, in all five branches. That is checked: the
/// tuple's SQL types have to line up with these fields and with each other, so a column
/// inserted in one branch and not the rest is a compile error. It replaced
/// `QueryableByName`, where the coupling was column *names* in a string and nothing
/// verified them at all.
#[derive(Debug, Clone, Queryable)]
pub struct WalletTransactionProjection {
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
    /// When the booking behind a host's charge ends, or `None` for a row that cannot
    /// ripen — a renter's charge, either side of a refund, any payout.
    ///
    /// The **instant**, not the verdict. Whether it has settled is
    /// `policy::wallet::pending`, which compares it against `now - SETTLEMENT_SECS`.
    /// That comparison used to live in the `SELECT`, which meant the settlement cutoff
    /// had to be bound into a query with no other use for it, and nothing could test the
    /// rule without a database.
    pub settles_at: Option<DateTime<Utc>>,
    /// Pending on a *status* rather than a deadline: a withdrawal whose transfer is
    /// still in flight. False on every other kind, which ripen by clock or not at all.
    pub pending_now: bool,
    /// The spot's title. `None` on a payout, which is about no spot, and on a charge
    /// whose spot has not been projected here yet.
    pub title: Option<String>,
    /// The slots the booking holds, in the spot's own wall clock. `None` on a payout,
    /// which has no booking and whose UNION branch selects a null `jsonb` in its place.
    pub booked: Option<Booked>,
    /// The spot's IANA zone, which is what `booked` is written in. `None` wherever
    /// `booked` is.
    pub timezone: Option<String>,
}

/// A host's money, as `WalletRepository::balance` computes it.
///
/// Four figures where `BalanceResponse` sends two: `earned_cents` and `paid_out_cents`
/// are the arithmetic behind `available_cents`, which is what the live tests assert on
/// and what makes a wrong figure debuggable. They are not on the wire because no screen
/// renders them.
///
/// Derived on every read, never stored — which is what makes a refunded booking drop out
/// of a host's income with no compensating write anywhere.
#[derive(Debug, Clone)]
pub struct BalanceProjection {
    /// Withdrawable now: settled income minus what has already been taken out.
    pub available_cents: i64,
    /// Everything earned and settled, ever.
    pub earned_cents: i64,
    pub paid_out_cents: i64,
    /// Earned but not yet settled.
    pub pending_cents: i64,
}
