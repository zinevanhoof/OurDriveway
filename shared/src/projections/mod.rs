//! What view-service **reads**. One projection per route, named for the route.
//!
//! A projection is a group of columns and nothing else: `Queryable + Selectable`, no
//! `Serialize`. What goes on the wire is a `*Response` in `crate::responses::view`,
//! which every route has even where it would be field-for-field identical. The point of
//! the split is that the wire contract stops moving when a projection does — adding a
//! column to a read is then a change to one statement, not to a client.
//!
//! ## The audience is which projection you select, not a field you null out
//!
//! `responses::view` used to hold one shape per *aggregate* and cut fields per caller
//! after the query: `SpotBooking::amount` was `Option<i64>` where `None` meant "you may
//! not see this", decided by an `is_party` helper the repository ran over every row.
//!
//! That rule is the type now. [`booking::PublicBookingProjection`] has no `amount` column
//! in its SELECT at all; [`booking::HostBookingProjection`] has an `amount: i64`.
//! Reaching the host projection *is* the permission decision, made once by the route
//! namespace that chose which repository function to call. So:
//!
//! **Every `Option` here means "there is nothing", never "you may not see this."**
//!
//! An `Option` that can never be `None` is the same lie wearing a different hat — see
//! [`user::AccountProjection::email`], which is a `String` because every projected row
//! has one.
//!
//! ## Duplication between projections is fine. One type is not.
//!
//! [`spot::PublicSpotProjection`] and [`spot::HostSpotProjection`] overlap by seven
//! columns and are deliberately two types. `spot` has **no** field-level scoping: every
//! column is visible to anyone who may see the row at all, so what separates these two is
//! which columns a screen reads, and a shared parent type would only make each of them
//! carry the other's.
//!
//! [`user::UserPublicProjection`] is the exception and the reason the rule is worth
//! stating: `app_user` *does* have a scoped subset, so that one type is shared, embedded
//! everywhere a person appears, and must never quietly grow an `email`.
//!
//! ## Columns match by position
//!
//! There is no aliasing anywhere and nothing to remember. The select clause is matched by
//! POSITION, so a nested projection reads whichever columns its slot in the tuple was
//! given — the same `UserPublicProjection` is the host in one slot and the renter in the
//! next, and two aliases of `app_user` in one statement are ordinary. A LEFT JOIN that
//! found nothing is `Option::<T>::as_select()`, which is `None` rather than a decode
//! error, and the clause is checked against the FROM at compile time.

pub mod booking;
pub mod spot;
pub mod user;
pub mod wallet;
