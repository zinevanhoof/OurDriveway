use chrono::{DateTime, Utc};
use geo::Point;
use surrealdb::types::{Datetime, SurrealValue};
use uuid::Uuid;

use surrealdb::types::vars;
use surrealdb::{engine::remote::ws::Client, method::Query};

use crate::{
    events::spot::{SpotCreated, SpotUpdated},
    general_models::spot::{Address, Availability},
};

/// The `spot` table in the read model — what a map query returns.
///
/// **One column is missing from this model on purpose** and must survive a write
/// from this stream: `owner`, the `record<user>` link, resolved by a subquery that
/// yields NONE until the owner's `UserRegistered` has been applied. See the module
/// doc.
///
/// So writes here go through `ViewSpotRepository::merge` and **never** a whole-row
/// `CONTENT`: that would erase it. The projectors advance independently, so on any
/// cold rebuild a `SpotCreated` can land after that spot's owner.
#[derive(Clone, Debug, SurrealValue)]
pub struct ViewSpot {
    pub id: Uuid,
    /// The plain uuid, which is what permissions and filters use — `owner` is only
    /// for nested GraphQL traversal.
    pub owner_id: Uuid,
    pub title: String,
    pub description: Option<String>,
    /// EUR cents.
    pub price_per_hour: i64,
    pub images: Vec<String>,
    /// x = lng, y = lat. Bound as a geometry value directly rather than assembled
    /// with `type::point([$lng, $lat])` — a `geo::Point<f64>` *is* a geometry point
    /// to the driver.
    pub location: Point<f64>,
    pub active: bool,
    /// Soft delete. The row stays selectable so `booking.spot` still resolves for a
    /// renter's past bookings — the lists filter this, the permission does not.
    pub deleted: bool,
    pub address: Address,
    pub availability: Availability,
    pub timezone: String,
    pub created_at: Datetime,
    pub updated_at: Datetime,
}

/// A partial update to a [`ViewSpot`], written with the struct-update idiom.
///
/// **`owner` is not a field here**, which is what makes "a SPOTS write can never
/// disturb the link" unrepresentable rather than merely untested. It is
/// `ViewSpotRepository::link_owner`'s, and the USERS stream's business.
#[derive(Debug, Default)]
pub struct ViewSpotPatch {
    pub owner_id: Option<Uuid>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub price_per_hour: Option<i64>,
    pub images: Option<Vec<String>>,
    pub location: Option<Point<f64>>,
    pub active: Option<bool>,
    pub deleted: Option<bool>,
    pub address: Option<Address>,
    pub availability: Option<Availability>,
    pub timezone: Option<String>,
    pub created_at: Option<Datetime>,
    pub updated_at: Option<Datetime>,
}

impl ViewSpotPatch {
    /// Binds every patchable column. Absent ones bind as NONE, which the
    /// `?? column` in `ViewSpotRepository::merge` and `::patch` turns into "leave it
    /// alone" — and on a row being created, into the schema's own default.
    pub fn bind(self, q: Query<'_, Client>) -> Query<'_, Client> {
        q.bind(vars! {
            owner_id:       self.owner_id,
            title:          self.title,
            description:    self.description,
            price_per_hour: self.price_per_hour,
            images:         self.images,
            location:       self.location,
            active:         self.active,
            deleted:        self.deleted,
            address:        self.address,
            availability:   self.availability,
            timezone:       self.timezone,
            created_at:     self.created_at,
            updated_at:     self.updated_at,
        })
    }

    /// Everything a `SpotCreated` carries. Every field is `Some`, so merging this
    /// over an existing row replaces all of them — while leaving `owner` alone,
    /// which is the point.
    pub fn created(e: SpotCreated, at: DateTime<Utc>) -> Self {
        Self {
            owner_id: Some(e.owner_id),
            title: Some(e.title),
            description: e.description,
            price_per_hour: Some(e.price_per_hour_cents),
            images: Some(e.images),
            location: Some(Point::new(e.lng, e.lat)),
            active: Some(true),
            deleted: Some(false),
            address: Some(e.address),
            availability: Some(e.availability),
            timezone: Some(e.timezone),
            created_at: Some(at.into()),
            updated_at: Some(at.into()),
        }
    }

    /// An edit. Absent stays absent, so "None means unchanged" survives into the
    /// read model.
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

    /// A soft delete, which is also a deactivation — so every list that already
    /// filters `active` hides it without learning a second field.
    pub fn deleted(at: DateTime<Utc>) -> Self {
        Self {
            active: Some(false),
            deleted: Some(true),
            updated_at: Some(at.into()),
            ..Self::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The SQL lives in `ViewSpotRepository` now, so nothing here can assert on it.
    /// What this *can* do is fail the moment the struct grows a field.
    ///
    /// The literal is **exhaustive on purpose** — no `..Default::default()`. Add a
    /// field to [`ViewSpotPatch`] and this stops compiling, which is the reminder
    /// that `bind` and the `SET` lists in `merge` and `patch` need it too.
    ///
    /// `owner` and `booked` are absent by construction, so the two columns this
    /// stream must never write are not writable from here at all. That used to be a
    /// test asserting they stayed out of `PATCH_SET`; the compiler holds it now.
    #[test]
    fn set_covers_every_patchable_column() {
        let _: ViewSpotPatch = ViewSpotPatch {
            owner_id: None,
            title: None,
            description: None,
            price_per_hour: None,
            images: None,
            location: None,
            active: None,
            deleted: None,
            address: None,
            availability: None,
            timezone: None,
            created_at: None,
            updated_at: None,
        };
    }

    /// A delete must set both flags; the live switch must not touch `deleted`, or
    /// re-listing a spot would undelete it.
    #[test]
    fn delete_also_deactivates() {
        let at = Utc::now();
        assert_eq!(ViewSpotPatch::deleted(at).active, Some(false));
        assert_eq!(ViewSpotPatch::deleted(at).deleted, Some(true));
        assert_eq!(
            ViewSpotPatch::updated(SpotUpdated::live(true), at).deleted,
            None
        );
    }
}
