//! What view-service reads and answers with — one type per (shape, audience).
//!
//! A projection is **both the row and the response**. It derives `FromRow` and
//! `Serialize`, so a read is `query_as::<_, PublicViewSpot>(…)` and the result is
//! handed to `Json` untouched. There is no row struct, no `From` impl and no mapping
//! closure between the database and the wire.
//!
//! ## The audience is the type, not a field
//!
//! `responses::view` used to hold one shape per *aggregate* and cut fields per caller
//! after the query: `SpotBooking::amount` was `Option<i64>` where `None` meant "you may
//! not see this", decided by an `is_party` helper the repository ran over every row.
//!
//! That rule now lives in the type. `PublicViewBooking` has no `amount` column in its
//! SELECT at all; `OwnerViewBooking` has an `amount: i64`. Reaching the owner type *is*
//! the permission decision, made once by the route that chose which repository function
//! to call. So:
//!
//! **Every `Option` here means "there is nothing", never "you may not see this."**
//!
//! An `Option` that can never be `None` is the same lie wearing a different hat — see
//! `OwnerViewUser::email`, which is a `String` because every projected row has one.
//!
//! ## Column aliasing
//!
//! `#[sqlx(flatten)]` reads the nested type's own field names out of the *same* row and
//! has no per-site prefix, so a nested projection sharing a name with its parent (`id`,
//! `title`) would silently read the parent's column.
//!
//! Nothing is dropped to dodge that. Each nested type declares `#[sqlx(rename)]` and
//! every statement writes the matching `AS`:
//!
//! - a person is always `user_*`, **including where there is no join** — the `/me` read
//!   selects `id AS user_id, …` so [`user::PublicViewUser`] decodes identically as
//!   owner-on-spot, renter-on-booking, and standalone;
//! - a spot nested in a booking row is always `spot_*`.
//!
//! `#[sqlx(rename)]` is SQL-side only. `#[serde(rename_all = "camelCase")]` still uses
//! the Rust field names, so the JSON is unaffected by any of it.
//!
//! **Ceiling:** one person per row. A statement needing an owner *and* a renter in the
//! same row needs a second type with its own prefix, because `flatten` cannot take one
//! per site. Nothing needs that today — the spot page reads its bookings separately.

pub mod booking;
pub mod payout;
pub mod spot;
pub mod user;

use serde::Serialize;
use sqlx::{FromRow, postgres::PgRow};

/// A LEFT JOIN that may have found nothing.
///
/// `owner_id`, `spot_id` and `renter_id` carry no foreign key on purpose — the streams
/// have no cross-stream ordering, so a spot is routinely projected before the user who
/// owns it. The join then finds no row, and that is ordinary rather than an error.
///
/// `#[sqlx(flatten)]` cannot express it: there is no `impl FromRow for Option<T>`, and
/// the derive calls `T::from_row` unconditionally, so a missed join arrives as a decode
/// error rather than a `None`. The optionality lives here instead of being rebuilt by
/// hand at every join site.
///
/// Only `ColumnDecode` becomes `None` — a NULL where the projection wants a value.
/// **`ColumnNotFound` still propagates**, because that is a statement missing an `AS`,
/// which is a bug in the query rather than a row that has not arrived.
#[derive(Debug, Clone, Serialize)]
#[serde(transparent)]
pub struct MaybeJoined<T>(pub Option<T>);

impl<T> MaybeJoined<T> {
    /// The joined value, if the join found one.
    pub fn as_ref(&self) -> Option<&T> {
        self.0.as_ref()
    }
}

impl<'r, T> FromRow<'r, PgRow> for MaybeJoined<T>
where
    T: FromRow<'r, PgRow>,
{
    fn from_row(row: &'r PgRow) -> sqlx::Result<Self> {
        match T::from_row(row) {
            Ok(v) => Ok(Self(Some(v))),
            // The join found nothing: a NOT NULL field of `T` came back NULL.
            Err(sqlx::Error::ColumnDecode { .. }) => Ok(Self(None)),
            Err(e) => Err(e),
        }
    }
}
