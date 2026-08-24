use chrono::{DateTime, Utc};
use geo::Point;
use surrealdb::types::{Datetime, SurrealValue};
use uuid::Uuid;

use surrealdb::types::vars;
use surrealdb::{engine::remote::ws::Client, method::Query};

use crate::{
    events::spot::{SpotCreated, SpotUpdated},
    general_models::spot::{Address, Availability},
    rpc::spot::SpotCard,
};

/// The `spot` table, whole.
///
/// Read entire rather than per-use-case: this replaced a `SpotForUpdate` that
/// selected three columns for the ownership check and a separate `card` query
/// that selected three others — two shapes and two round trips over one row that
/// is read by primary key either way.
///
/// Carries no methods beyond the event conversions below. Reading and writing it
/// is `SpotRepository`'s job.
#[derive(Clone, Debug, SurrealValue)]
pub struct Spot {
    /// Stored as `spot:⟨uuid⟩`; every read unwraps it back to a plain uuid with
    /// `record::id(id) AS id`.
    pub id: Uuid,
    /// A plain uuid column, not a record link — see `spot_owner` in
    /// `schemas/spot-schema.surql`, and the note there about why this is set by
    /// the service from the verified claim rather than by `VALUE $token.ID`.
    pub owner_id: Uuid,
    /// Read, never recomputed. `shard_of` would agree today, but this selects the
    /// subject the spot's whole history lives on, so a changed SHARD_COUNT would
    /// send its next event where no reader is looking.
    pub shard: String,
    pub title: String,
    pub description: Option<String>,
    /// EUR cents. Named for its column, which predates the `_cents` suffix the
    /// events use; never a float, because this feeds what a renter is charged.
    pub price_per_hour: i64,
    /// Absolute URLs. Rows written before the column existed hold NONE, which will
    /// not deserialize into a Vec — hence `images ?? []` in every statement that
    /// selects this table.
    pub images: Vec<String>,
    /// x = lng, y = lat, matching geo's convention and the argument order of the
    /// `type::point([$lng, $lat])` this replaced.
    ///
    /// Bound as a geometry value directly rather than assembled in SurrealQL: a
    /// `geo::Point<f64>` *is* `Value::Geometry(Geometry::Point(_))` to the driver,
    /// so there is nothing for a CONTENT body to construct.
    pub location: Point<f64>,
    pub active: bool,
    /// Permanent, unlike `active`. The row survives so a renter's past bookings
    /// still resolve a title and an address; every list filters on this.
    ///
    /// `deleted ?? false` when selected, for rows written before the field existed.
    pub deleted: bool,
    pub address: Address,
    pub availability: Availability,
    /// IANA name, derived from the geocoded point at creation.
    pub timezone: String,
    pub created_at: Datetime,
    pub updated_at: Datetime,
}

impl Spot {
    /// The row a `Created` writes.
    ///
    /// `at` is the envelope's clock, never this process's — every replica has to
    /// store the same timestamps for the same event. The three columns the event
    /// does not carry (`active`, `deleted`, and `created_at` equalling
    /// `updated_at`) are defaults of the model; they used to be literals in the
    /// projector's CONTENT block.
    pub fn created(e: SpotCreated, at: DateTime<Utc>) -> Self {
        Self {
            id: e.spot_id,
            owner_id: e.owner_id,
            shard: e.shard,
            title: e.title,
            description: e.description,
            price_per_hour: e.price_per_hour_cents,
            images: e.images,
            location: Point::new(e.lng, e.lat),
            active: true,
            deleted: false,
            address: e.address,
            availability: e.availability,
            timezone: e.timezone,
            created_at: at.into(),
            updated_at: at.into(),
        }
    }
}

/// A partial update to a [`Spot`], written with the struct-update idiom:
///
/// ```ignore
/// SpotPatch { active: Some(false), ..Default::default() }
/// ```
///
/// Only the columns an edit can touch. `owner_id`, `shard`, `location`, `address`,
/// `timezone` and `created_at` are written once by [`Spot::created`] and are not
/// representable here — a spot cannot change hands or move.
#[derive(Debug, Default)]
pub struct SpotPatch {
    pub title: Option<String>,
    pub description: Option<String>,
    pub price_per_hour: Option<i64>,
    pub images: Option<Vec<String>>,
    pub availability: Option<Availability>,
    pub active: Option<bool>,
    pub deleted: Option<bool>,
    pub updated_at: Option<Datetime>,
}

impl SpotPatch {
    /// Binds every patchable column. Absent ones bind as NONE, which the
    /// `?? column` in `SpotRepository::patch` turns into "leave it alone".
    ///
    /// Every field here has to appear in that statement's `SET` list, and vice
    /// versa.
    pub fn bind(self, q: Query<'_, Client>) -> Query<'_, Client> {
        q.bind(vars! {
            title:          self.title,
            description:    self.description,
            price_per_hour: self.price_per_hour,
            images:         self.images,
            availability:   self.availability,
            active:         self.active,
            deleted:        self.deleted,
            updated_at:     self.updated_at,
        })
    }

    /// An edit. Absent fields stay absent — the `?? column` in the patch statement
    /// is what makes "None means unchanged" hold in the projection as well as the
    /// event.
    pub fn updated(e: SpotUpdated, at: DateTime<Utc>) -> Self {
        Self {
            title: e.title,
            description: e.description,
            price_per_hour: e.price_per_hour_cents,
            images: e.images,
            availability: e.availability,
            active: e.active,
            updated_at: Some(at.into()),
            ..Self::default()
        }
    }

    /// A soft delete, which is also a deactivation.
    ///
    /// Setting `active = false` here rather than leaving it to the caller is the
    /// point: every guard that already checks the live switch then refuses a
    /// deleted spot without having learned about a second field.
    pub fn deleted(at: DateTime<Utc>) -> Self {
        Self {
            active: Some(false),
            deleted: Some(true),
            updated_at: Some(at.into()),
            ..Self::default()
        }
    }
}

/// What another service needs to *label* a spot — see [`crate::rpc::spot`].
///
/// Deliberately lossy. The card is one line on somebody else's screen, and
/// narrowing here is what keeps the RPC safe to answer at all.
impl From<Spot> for SpotCard {
    fn from(s: Spot) -> Self {
        Self {
            title: s.title,
            address: s.address.formatted,
            images: s.images,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The SQL lives in `SpotRepository` now, so nothing here can assert on it.
    /// What this *can* do is fail the moment the struct grows a field.
    ///
    /// The literal is **exhaustive on purpose** — no `..Default::default()`. Add a
    /// field to [`SpotPatch`] and this stops compiling, which is the reminder that
    /// `bind` and the `SET` list in `patch` need it too.
    #[test]
    fn set_covers_every_patchable_column() {
        let _: SpotPatch = SpotPatch {
            title: None,
            description: None,
            price_per_hour: None,
            images: None,
            availability: None,
            active: None,
            deleted: None,
            updated_at: None,
        };
    }

    /// The two states a spot can be "off" in are not interchangeable, and the
    /// delete patch has to set both — a guard that only knows `active` must still
    /// refuse a deleted spot.
    #[test]
    fn delete_also_deactivates() {
        let at = Utc::now();
        assert_eq!(SpotPatch::deleted(at).active, Some(false));
        assert_eq!(SpotPatch::deleted(at).deleted, Some(true));
        // The live switch is an edit now, and an edit must NOT touch `deleted` —
        // flipping a spot back on would otherwise undelete it.
        assert_eq!(
            SpotPatch::updated(SpotUpdated::live(true), at).deleted,
            None
        );
    }
}
