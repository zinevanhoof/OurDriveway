use surrealdb::types::SurrealValue;
use uuid::Uuid;

use surrealdb::types::vars;
use surrealdb::{engine::remote::ws::Client, method::Query};

use crate::{
    events::spot::{SpotCreated, SpotUpdated},
    general_models::spot::Availability,
};

/// booking-service's `spot` table — a **mirror**, not the spot.
///
/// A different table in a different database from
/// [`crate::domain_models::spot::Spot`], sharing only a name. That one is
/// spot-service's whole listing; this one is the handful of columns reserve needs
/// to authorize and price a booking server-side, plus one column no SPOTS event
/// ever carries: [`Self::bookings_seq`], derived from the BOOKINGS stream.
///
/// **Every SPOTS-owned column is `Option`.** The two projectors advance
/// independently, so on a cold rebuild a BOOKINGS event can arrive for a spot
/// whose `SpotCreated` has not been applied and create this row first. Reserve
/// rejects a spot whose availability is still absent, so the gap fails closed
/// rather than booking against nothing.
///
/// That ordering is also why every write here is `SpotMirrorRepository::merge`
/// and never `upsert`: `CONTENT` would erase `bookings_seq` whenever a SPOTS event
/// landed after a BOOKINGS one.
///
/// There is deliberately no `booked` column. Which slots are taken is a query over
/// the booking rows (`BookingRepository::taken_for_spot`), not a map kept in step
/// with them.
#[derive(Clone, Debug, SurrealValue)]
pub struct SpotMirror {
    pub id: Uuid,
    pub owner_id: Option<Uuid>,
    /// EUR cents.
    pub price_per_hour: Option<i64>,
    pub availability: Option<Availability>,
    /// IANA name. A booking's `booked` is bare wall-clock strings in this zone, so
    /// a cancel deadline cannot be placed on a timeline without it.
    pub timezone: Option<String>,
    /// The host's live switch. Defaulted rather than optional — a row created by
    /// the BOOKINGS side is bookable-if-known, and reserve refuses `false`.
    pub active: bool,
    /// Withdrawn for good. Separate from `active` because the two guard different
    /// things: the switch stops new reservations, a delete also stops a hold taken
    /// *before* it from ever being confirmed.
    ///
    /// `deleted ?? false` in every statement that selects this table — a row the
    /// BOOKINGS side created has no such column yet.
    pub deleted: bool,
    /// Stream sequence of the last BOOKINGS event applied for this spot — what
    /// reserve asserts as `Nats-Expected-Last-Subject-Sequence`.
    ///
    /// Advanced by `SpotMirrorRepository::advance` in the same transaction as the
    /// booking row the event wrote, so the cursor can never run ahead of the rows
    /// reserve reads — a cursor ahead of its data would authorise a booking against
    /// slots it has not seen.
    ///
    /// `bookings_seq ?? 0` when selected: on a cold rebuild the SPOTS side may have
    /// created this row before any BOOKINGS event landed on it.
    pub bookings_seq: u64,
}

/// A partial update to a [`SpotMirror`], written with the struct-update idiom:
///
/// ```ignore
/// SpotMirrorPatch { active: Some(false), ..Default::default() }
/// ```
///
/// **Only the SPOTS-owned columns.** `bookings_seq` is deliberately absent, which
/// makes "a SPOTS event can never clobber the compare-and-swap cursor"
/// unrepresentable rather than merely untested — it is written only by
/// `SpotMirrorRepository::advance`, in its own statement.
///
/// `id` is absent for the same reason it is on every other patch: the record key
/// addresses the row, it is not a column you assign.
#[derive(Debug, Default)]
pub struct SpotMirrorPatch {
    pub owner_id: Option<Uuid>,
    pub price_per_hour: Option<i64>,
    pub availability: Option<Availability>,
    pub timezone: Option<String>,
    pub active: Option<bool>,
    pub deleted: Option<bool>,
}

impl SpotMirrorPatch {
    /// Binds every patchable column. Absent ones bind as NONE, which the
    /// `?? column` in `SpotMirrorRepository::merge` turns into "leave it alone" —
    /// and on a row being created, into the schema's own default.
    ///
    /// Every field here has to appear in that statement's `SET` list, and vice
    /// versa.
    pub fn bind(self, q: Query<'_, Client>) -> Query<'_, Client> {
        q.bind(vars! {
            owner_id:       self.owner_id,
            price_per_hour: self.price_per_hour,
            availability:   self.availability,
            timezone:       self.timezone,
            active:         self.active,
            deleted:        self.deleted,
        })
    }

    /// What a `SpotCreated` mirrors. Only the columns reserve needs — the title,
    /// the photos and the location stay in spot-service.
    ///
    /// `active: true` is stated rather than left to the schema default, because
    /// this may well be patching a row the BOOKINGS side already created.
    pub fn created(e: SpotCreated) -> Self {
        Self {
            owner_id: Some(e.owner_id),
            price_per_hour: Some(e.price_per_hour_cents),
            availability: Some(e.availability),
            timezone: Some(e.timezone),
            active: Some(true),
            ..Self::default()
        }
    }

    /// An edit. Only price and availability are mirrored; the rest of a
    /// `SpotUpdated` is presentation this service has no use for.
    ///
    /// Absent stays absent, so "None means unchanged" survives into the mirror.
    pub fn updated(e: SpotUpdated) -> Self {
        Self {
            price_per_hour: e.price_per_hour_cents,
            availability: e.availability,
            active: e.active,
            ..Self::default()
        }
    }

    /// A soft delete, which is also a deactivation — same rule as the spot-service
    /// row it mirrors, so a guard that only checks `active` still refuses.
    pub fn deleted() -> Self {
        Self {
            active: Some(false),
            deleted: Some(true),
            ..Self::default()
        }
    }
}

/// Whether this mirror knows enough about the spot to price and authorize a
/// booking.
///
/// Returns the three columns together or nothing at all: they arrive in one event,
/// so a row holding some but not others is not a state the log can produce, and
/// callers that checked them one at a time would each invent their own answer for
/// the partial case.
impl SpotMirror {
    pub fn bookable(&self) -> Option<(&Availability, i64, Uuid)> {
        Some((
            self.availability.as_ref()?,
            self.price_per_hour?,
            self.owner_id?,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The SQL lives in `SpotMirrorRepository` now, so nothing here can assert on
    /// it. What this *can* do is fail the moment the struct grows a field.
    ///
    /// The literal is **exhaustive on purpose** — no `..Default::default()`. Add a
    /// field to [`SpotMirrorPatch`] and this stops compiling, which is the
    /// reminder that `bind` and the `SET` list in `merge` need it too.
    ///
    /// A mirror write must never be able to clear the compare-and-swap cursor.
    /// That used to be a test asserting every constructor left it absent; now
    /// `bookings_seq` is simply not a field here, so the compiler enforces it and
    /// the test is gone.
    #[test]
    fn set_covers_every_patchable_column() {
        let _: SpotMirrorPatch = SpotMirrorPatch {
            owner_id: None,
            price_per_hour: None,
            availability: None,
            timezone: None,
            active: None,
            deleted: None,
        };
    }

    #[test]
    fn delete_also_deactivates() {
        assert_eq!(SpotMirrorPatch::deleted().active, Some(false));
        assert_eq!(SpotMirrorPatch::deleted().deleted, Some(true));
        // The live switch is an edit now, and it must not undelete.
        assert_eq!(
            SpotMirrorPatch::updated(SpotUpdated::live(true)).deleted,
            None
        );
    }
}
