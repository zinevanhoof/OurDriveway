use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::{
    events::spot::{SpotCreated, SpotUpdated},
    general_models::spot::{Address, Availability},
};

/// The `spot` table in the read model — what a map query returns.
///
/// There is no missing column any more. This model used to omit `owner`, the
/// `record<user>` link, which meant writes had to go through `merge` and never a
/// whole-row `CONTENT` — a `CONTENT` would erase a link the USERS stream owned, and
/// the projectors advance independently so a `SpotCreated` routinely lands before its
/// owner. The link is gone: `owner_id` is a plain uuid and a read LEFT JOINs to
/// resolve the profile, so there is nothing a write here can clear.
#[derive(Clone, Debug, sqlx::FromRow)]
pub struct ViewSpot {
    pub id: Uuid,
    pub owner_id: Uuid,
    pub title: String,
    pub description: Option<String>,
    /// EUR cents.
    pub price_per_hour: i64,
    pub images: Vec<String>,
    /// Two `double precision` columns rather than a geometry.
    ///
    /// This was a `geo::Point<f64>`, which *was* a geometry point to the SurrealDB
    /// driver. PostGIS is unavailable on YSQL, so the point is stored as its
    /// components and the radius query is a bounding box on `spot_bbox` followed by
    /// an exact haversine — see `migrations/view/0001_init.sql`. lng before lat
    /// everywhere, matching geo's x/y and the argument order of the events.
    pub lng: f64,
    pub lat: f64,
    pub active: bool,
    /// Soft delete. The row stays selectable so a renter's past booking still
    /// resolves a title and an address — the lists filter this.
    pub deleted: bool,
    #[sqlx(json)]
    pub address: Address,
    #[sqlx(json)]
    pub availability: Availability,
    pub timezone: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A partial update to a [`ViewSpot`], written with the struct-update idiom.
#[derive(Debug, Default)]
pub struct ViewSpotPatch {
    pub owner_id: Option<Uuid>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub price_per_hour: Option<i64>,
    pub images: Option<Vec<String>>,
    pub lng: Option<f64>,
    pub lat: Option<f64>,
    pub active: Option<bool>,
    pub deleted: Option<bool>,
    pub address: Option<Address>,
    pub availability: Option<Availability>,
    pub timezone: Option<String>,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
}

impl ViewSpotPatch {
    // No `bind` — see the note in `domain_models::user::user`. sqlx binds
    // positionally, so the binds live beside the `$n` placeholders in
    // `ViewSpotRepository::merge` and `::patch`.

    /// Everything a `SpotCreated` carries. Every field is `Some`, so merging this
    /// over an existing row replaces all of them.
    pub fn created(e: SpotCreated, at: DateTime<Utc>) -> Self {
        Self {
            owner_id: Some(e.owner_id),
            title: Some(e.title),
            description: e.description,
            price_per_hour: Some(e.price_per_hour_cents),
            images: Some(e.images),
            lng: Some(e.lng),
            lat: Some(e.lat),
            active: Some(true),
            deleted: Some(false),
            address: Some(e.address),
            availability: Some(e.availability),
            timezone: Some(e.timezone),
            created_at: Some(at),
            updated_at: Some(at),
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
            updated_at: Some(at),
            ..Self::default()
        }
    }

    /// A soft delete, which is also a deactivation — so every list that already
    /// filters `active` hides it without learning a second field.
    pub fn deleted(at: DateTime<Utc>) -> Self {
        Self {
            active: Some(false),
            deleted: Some(true),
            updated_at: Some(at),
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
    /// `booked` is absent by construction — availability is a query over the booking
    /// rows, not a column on this one. `owner` used to be absent for a sharper reason:
    /// it was a record link the USERS stream owned, and a SPOTS write that touched it
    /// would erase it. There is no link now, so only `booked` is left out.
    #[test]
    fn set_covers_every_patchable_column() {
        let _: ViewSpotPatch = ViewSpotPatch {
            owner_id: None,
            title: None,
            description: None,
            price_per_hour: None,
            images: None,
            lng: None,
            lat: None,
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
