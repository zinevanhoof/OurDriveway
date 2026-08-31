use chrono::{DateTime, Utc};
use shared::domain_models::view::spot::ViewSpotPatch;
use shared::error::myerror::MyResult;
use shared::projections::spot::{OwnerViewSpot, PublicViewSpot, SpotListItem};
use sqlx::{PgExecutor, types::Json};
use uuid::Uuid;

use crate::policy::geo;
use crate::repository::booking_repository::ViewBookingRepository;

/// The `spot` table in the read model.
///
/// **There is no select rule to remember.** It used to be
/// `(active OR owner_id = $caller)` appended to whichever read needed it — one statement
/// answering two audiences, with nothing failing if a new read forgot the clause. The
/// audience is now the *function*, and each one's `WHERE` is the narrowest thing that
/// can answer it:
///
/// | function | who | `WHERE` |
/// |---|---|---|
/// | [`find_public_by_id`] | any reader | `id = $1 AND active` |
/// | [`find_owner_by_id`] | the host | `id = $1 AND owner_id = $2` |
/// | [`find_all_by_owner_id`] | the host | `owner_id = $1 AND NOT deleted` |
/// | [`find_all_by_radius`] | any reader | `active AND NOT deleted AND owner_id <> $1` |
///
/// A new read cannot inherit half a rule, because there is no rule to inherit — it picks
/// a projection, and the projection's name says who may hold it.
///
/// [`find_public_by_id`]: ViewSpotRepository::find_public_by_id
/// [`find_owner_by_id`]: ViewSpotRepository::find_owner_by_id
/// [`find_all_by_owner_id`]: ViewSpotRepository::find_all_by_owner_id
/// [`find_all_by_radius`]: ViewSpotRepository::find_all_by_radius
///
/// The two writes differ only in the verb — whether a missing row is created. `merge`
/// applies a `SpotCreated`, `patch` applies an edit and is a no-op on a row that was
/// never created. Both `COALESCE` every column, so an edit that carries only `active`
/// leaves the other twelve alone.
pub struct ViewSpotRepository;

impl ViewSpotRepository {
    /// One active spot with its owner resolved, as a prospective renter sees it.
    ///
    /// **The caller is not bound at all.** That is the point of the split: this
    /// statement answers one audience, so there is no identity in it to compare and no
    /// field to cut afterwards.
    ///
    /// Two statements rather than a three-way join: one spot with N bookings would
    /// repeat every spot column N times. They are issued here rather than by the route
    /// so that `/spots/:id` stays one call, and the second is
    /// [`ViewBookingRepository::find_all_public_by_spot_id`] — the availability answer,
    /// which is what a prospective renter is really asking for.
    ///
    /// A `deleted` spot still resolves here on purpose: a renter's past booking has to
    /// keep rendering a title and an address. It is the *lists* that filter it out.
    pub async fn find_public_by_id(
        ex: impl PgExecutor<'_> + Copy,
        spot_id: Uuid,
        now: DateTime<Utc>,
    ) -> MyResult<Option<PublicViewSpot>> {
        let Some(mut spot) = sqlx::query_as::<_, PublicViewSpot>(
            "SELECT s.id, s.owner_id, s.title, s.description, s.price_per_hour,
                    s.images, s.lng, s.lat, s.active, s.address, s.availability,
                    s.timezone,
                    u.id              AS user_id,
                    u.first_name      AS user_first_name,
                    u.last_name       AS user_last_name,
                    u.profile_picture AS user_profile_picture
               FROM spot s
               LEFT JOIN app_user u ON u.id = s.owner_id
              WHERE s.id = $1 AND s.active",
        )
        .bind(spot_id)
        .fetch_optional(ex)
        .await?
        else {
            return Ok(None);
        };

        spot.bookings = ViewBookingRepository::find_all_public_by_spot_id(ex, spot_id, now).await?;
        Ok(Some(spot))
    }

    /// One spot as its host sees it, with every booking on it in full.
    ///
    /// `owner_id = $2` is the whole authorization: a non-owner matches no row and the
    /// route answers 404, which is also what a stranger asking about a spot that does
    /// not exist gets. An **inactive** spot resolves here and only here — that is what
    /// the live switch is for.
    ///
    /// Because ownership is settled by this statement, the second one
    /// ([`ViewBookingRepository::find_all_owner_by_spot_id`]) needs no caller and no
    /// status filter at all.
    pub async fn find_owner_by_id(
        ex: impl PgExecutor<'_> + Copy,
        spot_id: Uuid,
        owner_id: Uuid,
        now: DateTime<Utc>,
    ) -> MyResult<Option<OwnerViewSpot>> {
        let Some(mut spot) = sqlx::query_as::<_, OwnerViewSpot>(
            "SELECT s.id, s.owner_id, s.title, s.description, s.price_per_hour,
                    s.images, s.lng, s.lat, s.active, s.address, s.availability,
                    s.timezone,
                    u.id              AS user_id,
                    u.first_name      AS user_first_name,
                    u.last_name       AS user_last_name,
                    u.profile_picture AS user_profile_picture
               FROM spot s
               LEFT JOIN app_user u ON u.id = s.owner_id
              WHERE s.id = $1 AND s.owner_id = $2",
        )
        .bind(spot_id)
        .bind(owner_id)
        .fetch_optional(ex)
        .await?
        else {
            return Ok(None);
        };

        spot.bookings = ViewBookingRepository::find_all_owner_by_spot_id(ex, spot_id, now).await?;
        Ok(Some(spot))
    }

    /// The caller's own listings.
    ///
    /// Scoped to the owner, so the select rule is not appended: it admits an owner's
    /// inactive spots on purpose — that is what the live switch is for — and `owner_id
    /// = $1` is already narrower. `deleted` is filtered here instead, because a
    /// deleted row survives only so past bookings resolve, and must not appear in the
    /// host's own list.
    pub async fn find_all_by_owner_id(
        ex: impl PgExecutor<'_>,
        owner_id: Uuid,
    ) -> MyResult<Vec<SpotListItem>> {
        Ok(sqlx::query_as(
            "SELECT id, title, price_per_hour, images, active, lng, lat, address, availability
               FROM spot
              WHERE owner_id = $1 AND deleted = false
              ORDER BY created_at DESC",
        )
        .bind(owner_id)
        .fetch_all(ex)
        .await?)
    }

    /// Spots within `meters` of a point, **never including the caller's own**.
    ///
    /// Excluding the caller is unconditional rather than a parameter. `SPOTS_NEARBY`
    /// did it (`owner_id: { ne: $me }`) and `SPOTS_IN_RADIUS` did not, which meant the
    /// map showed a host their own driveway as somewhere to park. One rule is easier
    /// to state than two, and "somewhere to park" never means your own.
    ///
    /// Two stages, because there is no spatial index to ask directly:
    ///
    /// 1. `lat`/`lng` ranges, served by the partial index `spot_bbox`. This is the
    ///    part that does not read the table.
    /// 2. haversine over what stage 1 returns, in SQL so the rows never leave the
    ///    database.
    ///
    /// `LIMIT` is a backstop, not a page: a map viewport is bounded, and a caller who
    /// asks for a 500 km radius gets an arbitrary subset rather than the whole table.
    pub async fn find_all_by_radius(
        ex: impl PgExecutor<'_>,
        caller: Uuid,
        lng: f64,
        lat: f64,
        meters: f64,
    ) -> MyResult<Vec<SpotListItem>> {
        let (min_lat, max_lat, min_lng, max_lng) = geo::bbox(lng, lat, meters);

        // `caller` is a plain `Uuid`, not an `Option`: every view route is behind
        // `AuthedJwt`, so there is no anonymous reader to write a NULL branch for.
        Ok(sqlx::query_as(
            "SELECT id, title, price_per_hour, images, active, lng, lat, address, availability
               FROM spot
              WHERE deleted = false
                AND active
                AND owner_id <> $1
                AND lat BETWEEN $2 AND $3
                AND lng BETWEEN $4 AND $5
                AND 2 * 6371000 * asin(sqrt(
                        power(sin(radians(lat - $6) / 2), 2)
                      + cos(radians($6)) * cos(radians(lat))
                      * power(sin(radians(lng - $7) / 2), 2)
                    )) <= $8
              LIMIT 500",
        )
        .bind(caller)
        .bind(min_lat)
        .bind(max_lat)
        .bind(min_lng)
        .bind(max_lng)
        .bind(lat)
        .bind(lng)
        .bind(meters)
        .fetch_all(ex)
        .await?)
    }

    /// Apply a `SpotCreated`, creating the row if it is not there yet.
    ///
    /// Upsert rather than insert, so a redelivered `SpotCreated` is idempotent. Every
    /// column of `ViewSpotPatch::created` is `Some`, so this fills all of them.
    ///
    /// The `VALUES` row needs its own `COALESCE`s against the table defaults for
    /// `images`, `active` and `deleted`: those columns are `NOT NULL`, and a create is
    /// the one path that could otherwise write NULL into them.
    ///
    /// **The binds are positional** and must match the `$n`.
    pub async fn merge(
        ex: impl PgExecutor<'_>,
        spot_id: Uuid,
        patch: ViewSpotPatch,
    ) -> MyResult<()> {
        sqlx::query(
            "INSERT INTO spot
                 (id, owner_id, title, description, price_per_hour, images, lng, lat,
                  active, deleted, address, availability, timezone, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, COALESCE($6, '{}'), $7, $8,
                     COALESCE($9, true), COALESCE($10, false), $11, $12, $13, $14, $15)
             ON CONFLICT (id) DO UPDATE SET
                 owner_id       = COALESCE($2,  spot.owner_id),
                 title          = COALESCE($3,  spot.title),
                 description    = COALESCE($4,  spot.description),
                 price_per_hour = COALESCE($5,  spot.price_per_hour),
                 images         = COALESCE($6,  spot.images),
                 lng            = COALESCE($7,  spot.lng),
                 lat            = COALESCE($8,  spot.lat),
                 active         = COALESCE($9,  spot.active),
                 deleted        = COALESCE($10, spot.deleted),
                 address        = COALESCE($11, spot.address),
                 availability   = COALESCE($12, spot.availability),
                 timezone       = COALESCE($13, spot.timezone),
                 created_at     = COALESCE($14, spot.created_at),
                 updated_at     = COALESCE($15, spot.updated_at)",
        )
        .bind(spot_id)
        .bind(patch.owner_id)
        .bind(patch.title)
        .bind(patch.description)
        .bind(patch.price_per_hour)
        .bind(patch.images)
        .bind(patch.lng)
        .bind(patch.lat)
        .bind(patch.active)
        .bind(patch.deleted)
        .bind(patch.address.map(Json))
        .bind(patch.availability.map(Json))
        .bind(patch.timezone)
        .bind(patch.created_at)
        .bind(patch.updated_at)
        .execute(ex)
        .await?;
        Ok(())
    }

    /// Apply an edit. Does not create the row — an edit for a spot that was never
    /// created is a no-op, not a partial row.
    ///
    /// The same thirteen columns as [`Self::merge`], differing only in the verb. Two
    /// statements written out rather than one built from a flag: this file's whole
    /// convention is that a method's SQL is the string in front of you, and a reader
    /// asking "does an edit create the row?" should be able to answer it here rather
    /// than by following a boolean into a shared helper.
    ///
    /// They do have to be kept in step by hand. `a_spots_write_fills_its_own_columns_
    /// and_leaves_the_rest` in this module's `mod.rs` exercises both against a real
    /// database, which is what would catch a column added to one and not the other.
    ///
    /// **The binds are positional** and must match the `$n`.
    pub async fn patch(
        ex: impl PgExecutor<'_>,
        spot_id: Uuid,
        patch: ViewSpotPatch,
    ) -> MyResult<()> {
        sqlx::query(
            "UPDATE spot SET
                 owner_id       = COALESCE($2,  owner_id),
                 title          = COALESCE($3,  title),
                 description    = COALESCE($4,  description),
                 price_per_hour = COALESCE($5,  price_per_hour),
                 images         = COALESCE($6,  images),
                 lng            = COALESCE($7,  lng),
                 lat            = COALESCE($8,  lat),
                 active         = COALESCE($9,  active),
                 deleted        = COALESCE($10, deleted),
                 address        = COALESCE($11, address),
                 availability   = COALESCE($12, availability),
                 timezone       = COALESCE($13, timezone),
                 created_at     = COALESCE($14, created_at),
                 updated_at     = COALESCE($15, updated_at)
             WHERE id = $1",
        )
        .bind(spot_id)
        .bind(patch.owner_id)
        .bind(patch.title)
        .bind(patch.description)
        .bind(patch.price_per_hour)
        .bind(patch.images)
        .bind(patch.lng)
        .bind(patch.lat)
        .bind(patch.active)
        .bind(patch.deleted)
        .bind(patch.address.map(Json))
        .bind(patch.availability.map(Json))
        .bind(patch.timezone)
        .bind(patch.created_at)
        .bind(patch.updated_at)
        .execute(ex)
        .await?;
        Ok(())
    }
}

// `link_owner` is gone with the `owner` record link it maintained. `owner_id` is a
// plain uuid, and a read that wants the host's name LEFT JOINs `app_user` on it.
