use diesel::prelude::*;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use shared::domain_models::view::spot::ViewSpotPatch;
use shared::error::myerror::MyResult;
use shared::projections::spot::{
    HostSpotListProjection, HostSpotProjection, PublicSpotPinProjection, PublicSpotProjection,
};
use shared::schema::view::{app_user, spot};
use uuid::Uuid;

use crate::policy::geo;
use sql::{asin, cos, power, radians, sin, sqrt};

/// The Postgres maths the radius search needs, as typed diesel expressions.
///
/// `define_sql_function!` **declares** a function that already exists server-side; it
/// does not create one. All six are Postgres (and YSQL) built-ins, so this needs no
/// migration — it is only the Rust side learning their signatures.
///
/// Diesel ships `count`, `sum`, `coalesce` and the date/time family, and no trigonometry,
/// which is why these are written out. Six one-line declarations buy the whole haversine
/// as an expression tree instead of a `sql::<Bool>` string with four `.bind()`s spliced
/// through it: the column references are checked against `spot`, the argument types are
/// checked, and a typo is a compile error rather than a subtly wrong map.
mod sql {
    use diesel::sql_types::Double;

    diesel::define_sql_function! { fn radians(x: Double) -> Double }
    diesel::define_sql_function! { fn sin(x: Double) -> Double }
    diesel::define_sql_function! { fn cos(x: Double) -> Double }
    diesel::define_sql_function! { fn asin(x: Double) -> Double }
    diesel::define_sql_function! { fn sqrt(x: Double) -> Double }
    diesel::define_sql_function! { fn power(x: Double, y: Double) -> Double }
}

/// The `spot` table in the read model.
///
/// **There is no select rule to remember.** It used to be
/// `(active OR host_id = $caller)` appended to whichever read needed it — one statement
/// answering two audiences, with nothing failing if a new read forgot the clause. The
/// audience is the *route namespace* now, and each function's `WHERE` is that namespace's
/// one predicate and nothing else:
///
/// | function | namespace | `WHERE` |
/// |---|---|---|
/// | [`find_for_public`] | `public` | `id = $1 AND active` |
/// | [`find_pins_for_public`] | `public` | `active AND NOT deleted AND host_id <> $1` |
/// | [`find_for_host`] | `host` | `id = $1 AND host_id = $2` |
/// | [`find_list_for_host`] | `host` | `host_id = $1 AND NOT deleted` |
///
/// A new read cannot inherit half a rule, because there is no rule to inherit — it picks
/// a projection, and the projection's name says which namespace may hold it.
///
/// **Reads return a projection, never a response.** Assembling parent and children into
/// a `*Response` is the route's job; a repository that returned wire shapes would have to
/// know which fields a screen renders.
///
/// [`find_for_public`]: ViewSpotRepository::find_for_public
/// [`find_pins_for_public`]: ViewSpotRepository::find_pins_for_public
/// [`find_for_host`]: ViewSpotRepository::find_for_host
/// [`find_list_for_host`]: ViewSpotRepository::find_list_for_host
///
/// The two writes differ only in whether a missing row is created. `merge` applies a
/// `SpotCreated`; `patch` applies an edit and is a no-op on a row that was never created.
/// Both are derived from [`ViewSpotPatch`] — `Insertable` for the insert's column list,
/// `AsChangeset` for the `SET` — and both skip a `None`, which is the absent-is-unchanged
/// rule the fifteen `COALESCE`s they replaced used to spell out by hand.
///
/// Nothing in this file is `sql_query` any more.
pub struct ViewSpotRepository;

impl ViewSpotRepository {
    /// One active spot with its host resolved, as a prospective renter sees it.
    ///
    /// **The caller is not bound at all.** That is the point of the split: this statement
    /// answers one audience, so there is no identity in it to compare and no field to cut
    /// afterwards.
    ///
    /// The parent half only. The route issues
    /// [`ViewBookingRepository::find_for_public_spot`] against the row this returns, via
    /// `belonging_to` — see the note on two statements in `repository/mod.rs`.
    ///
    /// A `deleted` spot still resolves here on purpose: a renter's past booking has to
    /// keep rendering a title and an address. It is the *lists* that filter it out.
    ///
    /// [`ViewBookingRepository::find_for_public_spot`]:
    ///     crate::repository::booking_repository::ViewBookingRepository::find_for_public_spot
    pub async fn find_for_public(
        conn: &mut AsyncPgConnection,
        spot_id: Uuid,
    ) -> MyResult<Option<PublicSpotProjection>> {
        Ok(spot::table
            .left_join(app_user::table.on(app_user::id.eq(spot::host_id)))
            .filter(spot::id.eq(spot_id).and(spot::active))
            .select(PublicSpotProjection::as_select())
            .first(conn)
            .await
            .optional()?)
    }

    /// One spot as its host sees it. The parent half.
    ///
    /// `host_id = $2` is the whole authorization: a non-host matches no row and the
    /// route answers 404, which is also what a stranger asking about a spot that does not
    /// exist gets. An **inactive** spot resolves here and only here — that is what the
    /// live switch is for.
    ///
    /// Because ownership is settled by this statement, the child read against its result
    /// needs no caller and no status filter at all.
    ///
    /// No host join, unlike [`find_for_public`]: a host does not need their own name
    /// told back to them.
    ///
    /// [`find_for_public`]: ViewSpotRepository::find_for_public
    pub async fn find_for_host(
        conn: &mut AsyncPgConnection,
        spot_id: Uuid,
        host_id: Uuid,
    ) -> MyResult<Option<HostSpotProjection>> {
        Ok(spot::table
            .filter(spot::id.eq(spot_id).and(spot::host_id.eq(host_id)))
            .select(HostSpotProjection::as_select())
            .first(conn)
            .await
            .optional()?)
    }

    /// The caller's own listings.
    ///
    /// `host_id = $1` admits a host's inactive spots on purpose — that is what the
    /// live switch is for. `deleted` is filtered here instead, because a deleted row
    /// survives only so past bookings resolve, and must not appear in the host's own
    /// list.
    pub async fn find_list_for_host(
        conn: &mut AsyncPgConnection,
        host_id: Uuid,
    ) -> MyResult<Vec<HostSpotListProjection>> {
        Ok(spot::table
            .filter(spot::host_id.eq(host_id).and(spot::deleted.eq(false)))
            .order(spot::created_at.desc())
            .select(HostSpotListProjection::as_select())
            .load(conn)
            .await?)
    }

    /// Spots within `meters` of a point, **never including the caller's own**.
    ///
    /// Excluding the caller is unconditional rather than a parameter. `SPOTS_NEARBY` did
    /// it (`host_id: { ne: $me }`) and `SPOTS_IN_RADIUS` did not, which meant the map
    /// showed a host their own driveway as somewhere to park. One rule is easier to state
    /// than two, and "somewhere to park" never means your own.
    ///
    /// Two stages, because there is no spatial index to ask directly:
    ///
    /// 1. `lat`/`lng` ranges, served by the partial index `spot_bbox`. This is the part
    ///    that does not read the table.
    /// 2. haversine over what stage 1 returns, in SQL so the rows never leave the
    ///    database.
    ///
    /// `LIMIT` is a backstop, not a page: a map viewport is bounded, and a caller who
    /// asks for a 500 km radius gets an arbitrary subset rather than the whole table.
    pub async fn find_pins_for_public(
        conn: &mut AsyncPgConnection,
        caller: Uuid,
        lng: f64,
        lat: f64,
        meters: f64,
    ) -> MyResult<Vec<PublicSpotPinProjection>> {
        let (min_lat, max_lat, min_lng, max_lng) = geo::bbox(lng, lat, meters);

        // `caller` is a plain `Uuid`, not an `Option`: every view route is behind
        // `AuthedJwt`, so there is no anonymous reader to write a NULL branch for.
        // Stage one in the DSL — the bounding box is the part that decides how many rows
        // are read, and `spot_bbox` is the partial index that serves it.
        //
        // Stage two is the exact filter, as a typed expression tree — see the `sql`
        // module above for the six declarations that make it one. It reads as the formula
        // it is, and `spot::lat` / `spot::lng` are real column references rather than
        // table-qualified text inside a string that nothing checks.
        //
        // `geo::EARTH_RADIUS_M` and not a literal: `geo::haversine` is the reference this
        // must agree with, and it reads the same constant.
        //
        // Not `BoxableExpression`: nothing here is assembled at runtime. See the note in
        // `repository/mod.rs` for when that changes.
        Ok(spot::table
            .filter(
                spot::deleted
                    .eq(false)
                    .and(spot::active)
                    .and(spot::host_id.ne(caller))
                    .and(spot::lat.between(min_lat, max_lat))
                    .and(spot::lng.between(min_lng, max_lng))
                    .and(
                        // 2R·asin(√a) ≤ meters, where
                        // a = sin²(Δφ/2) + cos φ₁ · cos φ₂ · sin²(Δλ/2).
                        (asin(sqrt(
                            power(sin(radians(spot::lat - lat) / 2.0), 2.0)
                                + cos(radians(lat))
                                    * cos(radians(spot::lat))
                                    * power(sin(radians(spot::lng - lng) / 2.0), 2.0),
                        )) * (2.0 * geo::EARTH_RADIUS_M))
                            .le(meters),
                    ),
            )
            .limit(500)
            .select(PublicSpotPinProjection::as_select())
            .load(conn)
            .await?)
    }

    /// Apply a `SpotCreated`, creating the row if it is not there yet.
    ///
    /// Upsert rather than insert, so a redelivered `SpotCreated` is idempotent. Every
    /// column of `ViewSpotPatch::created` is `Some`, so this fills all of them.
    ///
    /// **`Insertable` is what makes the table's defaults apply.** It omits a `None` field
    /// from the insert's column list, and an omitted column is the only way a default is
    /// ever reached — a column that is *named* with a NULL bind gets NULL, which is why
    /// the hand-written form this replaced had to carry `COALESCE($6, '{}')`,
    /// `COALESCE($9, true)` and `COALESCE($10, false)` to put `images`, `active` and
    /// `deleted` back by hand.
    ///
    /// `AsChangeset` skips the same `None` on the conflict path, so the two derives are
    /// the whole statement. `a_spots_write_fills_its_own_columns_and_leaves_the_rest`
    /// merges a patch with those three cleared and asserts the defaults land.
    pub async fn merge(
        conn: &mut AsyncPgConnection,
        spot_id: Uuid,
        patch: ViewSpotPatch,
    ) -> MyResult<()> {
        diesel::insert_into(spot::table)
            .values((spot::id.eq(spot_id), &patch))
            .on_conflict(spot::id)
            .do_update()
            .set(&patch)
            .execute(conn)
            .await?;
        Ok(())
    }

    /// Apply an edit. Does not create the row — an edit for a spot that was never
    /// created is a no-op, not a partial row.
    ///
    /// **`AsChangeset`, unlike [`Self::merge`].** The two used to be the same fifteen
    /// positional binds under two verbs, kept in step by hand. They are not symmetric,
    /// and the asymmetry is the whole reason `merge` is still SQL: `AsChangeset`
    /// describes a `SET` clause, and `merge`'s problem is its `VALUES` clause — the
    /// `COALESCE($6, '{}')` / `COALESCE($9, true)` / `COALESCE($10, false)` that fill
    /// `NOT NULL` columns from the table's defaults on the insert path. There is nothing
    /// like that here.
    ///
    /// `AsChangeset` **skips a `None` field**, which is exactly what `COALESCE($n,
    /// column)` did, so absent-is-unchanged is unchanged — `a_spots_write_fills_its_own_
    /// columns_and_leaves_the_rest` in this module's `mod.rs` asserts it against a real
    /// database and passed without edit. The ceiling is unchanged too: no patch can set a
    /// column back to NULL, because "absent" and "null" are one value here.
    ///
    /// ponytail: an all-`None` patch is `Err(QueryBuilderError(EmptyChangeset))` where
    /// the `COALESCE` form was a harmless no-op `UPDATE`. Unreachable today — every
    /// `ViewSpotPatch` constructor sets at least `updated_at` — but a new constructor
    /// that can produce an empty patch would 500 the projector and wedge its lane. Guard
    /// it in the constructor if one ever appears; do not re-add the SQL.
    pub async fn patch(
        conn: &mut AsyncPgConnection,
        spot_id: Uuid,
        patch: ViewSpotPatch,
    ) -> MyResult<()> {
        diesel::update(spot::table.find(spot_id))
            .set(&patch)
            .execute(conn)
            .await?;
        Ok(())
    }
}

// `link_host` is gone with the `host` record link it maintained. `host_id` is a
// plain uuid, and a read that wants the host's name LEFT JOINs `app_user` on it.
