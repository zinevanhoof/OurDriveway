use uuid::Uuid;

use crate::{
    events::spot::{SpotCreated, SpotUpdated},
    general_models::spot::Availability,
};

/// booking-service's `spot` table — a **mirror**, not the spot.
///
/// A different table in a different database from
/// [`crate::domain_models::spot::Spot`], sharing only a name. That one is
/// spot-service's whole listing; this one is the handful of columns reserve needs
/// to authorize and price a booking server-side.
///
/// **Every SPOTS-owned column is `Option`**, and still is even though the BOOKINGS
/// side no longer writes this table at all. The reason changed rather than
/// disappeared: the streams expire after seven days, so a consumer built later can
/// legitimately see a `SpotUpdated` for a spot whose `SpotCreated` has already aged
/// out, and `merge` would create a partial row from it. Reserve rejects a spot whose
/// availability is absent, so the gap fails closed rather than booking against
/// nothing.
///
/// ## `bookings_seq` is gone
///
/// It held the stream sequence of the last BOOKINGS event applied for this spot, and
/// existed to *manufacture* a write conflict: two renters racing one slot insert two
/// different booking rows, so nothing collided, and bumping a shared counter on the
/// spot forced TiKV to refuse one of them.
///
/// Read Committed does not refuse that bump — the second writer blocks, re-reads and
/// applies — so the counter would have silently stopped working. Reserve takes
/// `SELECT … FOR UPDATE` on this row instead, and the fresh snapshot its next
/// statement gets is what makes the loser see the winner's booking. See
/// `migrations/booking/0001_init.sql` and `shared::db::next_version`.
///
/// With it went the reason writes here had to be `merge` and never a whole-row
/// write — there is no longer a column another stream owns. `merge` stays because
/// of the partial-row case above, not because of the cursor.
///
/// There is deliberately no `booked` column. Which slots are taken is a query over
/// the booking rows (`BookingRepository::taken_for_spot`), not a map kept in step
/// with them.
#[derive(Clone, Debug, sqlx::FromRow)]
pub struct SpotMirror {
    pub id: Uuid,
    pub owner_id: Option<Uuid>,
    /// EUR cents.
    pub price_per_hour: Option<i64>,
    /// `json(nullable)`, not plain `json`. This column is genuinely NULL on a
    /// half-built mirror — the case the whole `Option` here exists for — and plain
    /// `#[sqlx(json)]` decodes straight into `Availability`, failing with
    /// `UnexpectedNullError` rather than yielding `None`.
    #[sqlx(json(nullable))]
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
    pub deleted: bool,
}

/// A partial update to a [`SpotMirror`], written with the struct-update idiom:
///
/// ```ignore
/// SpotMirrorPatch { active: Some(false), ..Default::default() }
/// ```
///
/// Every column this table has, now that `bookings_seq` is gone — that one used to be
/// deliberately absent so a SPOTS event could not clobber the compare-and-swap cursor.
///
/// `id` is absent for the same reason it is on every other patch: the primary key
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
    // No `bind` — see the note in `domain_models::user::user`. sqlx binds
    // positionally, so the binds live beside the `$n` placeholders in
    // `SpotMirrorRepository::merge`.

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
    /// field to [`SpotMirrorPatch`] and this stops compiling, which is the reminder
    /// that the `SET` list in `merge` needs it too, and a `.bind()` in the matching
    /// position.
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
