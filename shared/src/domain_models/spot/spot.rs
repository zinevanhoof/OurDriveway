use chrono::{DateTime, Utc};
use diesel::prelude::*;
use uuid::Uuid;

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
#[derive(Clone, Debug, Queryable, Selectable, Insertable, AsChangeset)]
#[diesel(table_name = crate::schema::spot::spot)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct Spot {
    /// A plain uuid primary key — there is nothing left to unwrap, so `SELECT *`
    /// is enough where every read used to carry `record::id(id) AS id`.
    pub id: Uuid,
    /// Bumped by spot-service inside the transaction that writes this row. The
    /// version a client waits on, the key concurrent writers collide on, and the gap
    /// detector for an out-of-order event — see `shared::events::Envelope`.
    ///
    /// On the model rather than only in the schema so a whole-row write carries it,
    /// and so a read can answer "which version is this" — which is what a backfill
    /// re-emitting current state has to stamp on the events it raises.
    pub version: i64,
    /// A plain uuid column, not a record link — see `spot_host` in
    /// `schemas/spot-schema.surql`, and the note there about why this is set by
    /// the service from the verified claim rather than by `VALUE $token.ID`.
    pub host_id: Uuid,
    pub title: String,
    pub description: Option<String>,
    /// EUR cents. Named for its column, which predates the `_cents` suffix the
    /// events use; never a float, because this feeds what a renter is charged.
    pub price_per_hour: i64,
    /// Absolute URLs. `text[]`, `NOT NULL DEFAULT '{}'`, so the `images ?? []` that
    /// used to be in every statement selecting this table is gone.
    pub images: Vec<String>,
    /// Two `double precision` columns rather than a geometry.
    ///
    /// This was a `geo::Point<f64>`, which *was* `Value::Geometry(Geometry::Point(_))`
    /// to the SurrealDB driver. PostGIS is unavailable on YSQL, so the point is stored
    /// as its components — lng before lat, matching geo's x/y and the argument order
    /// the events already use.
    pub lng: f64,
    pub lat: f64,
    pub active: bool,
    /// Permanent, unlike `active`. The row survives so a renter's past bookings
    /// still resolve a title and an address; every list filters on this.
    ///
    pub deleted: bool,
    pub address: Address,
    pub availability: Availability,
    /// IANA name, derived from the geocoded point at creation.
    pub timezone: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Spot {
    /// The row a `Created` writes.
    ///
    /// `at` is the envelope's clock, never this process's — every replica has to
    /// store the same timestamps for the same event. The three columns the event
    /// does not carry (`active`, `deleted`, and `created_at` equalling
    /// `updated_at`) are defaults of the model; they used to be literals in the
    /// projector's CONTENT block.
    pub fn created(e: SpotCreated, at: DateTime<Utc>, version: i64) -> Self {
        Self {
            id: e.spot_id,
            version,
            host_id: e.host_id,
            title: e.title,
            description: e.description,
            price_per_hour: e.price_per_hour_cents,
            images: e.images,
            lng: e.lng,
            lat: e.lat,
            active: true,
            deleted: false,
            address: e.address,
            availability: e.availability,
            timezone: e.timezone,
            created_at: at,
            updated_at: at,
        }
    }
}

/// A partial update to a [`Spot`], written with the struct-update idiom:
///
/// ```ignore
/// SpotPatch { active: Some(false), ..Default::default() }
/// ```
///
/// Only the columns an edit can touch. `host_id`, `lng`/`lat`, `address`,
/// `timezone` and `created_at` are written once by [`Spot::created`] and are not
/// representable here — a spot cannot change hands or move.
#[derive(Debug, Default, AsChangeset)]
#[diesel(table_name = crate::schema::spot::spot)]
pub struct SpotPatch {
    pub title: Option<String>,
    pub description: Option<String>,
    pub price_per_hour: Option<i64>,
    pub images: Option<Vec<String>>,
    pub availability: Option<Availability>,
    pub active: Option<bool>,
    pub deleted: Option<bool>,
    pub updated_at: Option<DateTime<Utc>>,
}

impl SpotPatch {
    // No `bind` — `AsChangeset` is the `SET` list. See the note in
    // `domain_models::user::user`.

    /// An edit. Absent fields stay absent — the `COALESCE($n, column)` in the patch
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
            updated_at: Some(at),
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
            updated_at: Some(at),
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
    /// the `SET` list in `patch` needs it too, and a `.bind()` in the matching
    /// position.
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
