use chrono::{DateTime, Utc};
use diesel::prelude::*;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use shared::domain_models::view::booking::{ViewBooking, ViewBookingPatch};
use shared::error::myerror::MyResult;
use shared::projections::booking::{
    HostBookingProjection, PublicBookingProjection, RenterBookingProjection,
};
use shared::projections::spot::{HostSpotProjection, PublicSpotProjection};
use shared::schema::view::{app_user, booking};
use uuid::Uuid;

/// The `booking` table in the read model.
///
/// **What used to be one clause is now four functions**, one per namespace. Every read of
/// this table carried the same disjunction:
///
/// ```sql
/// (b.renter_id = $1 OR b.host_id = $1 OR b.status IN ('reserved', 'confirmed'))
/// ```
///
/// — one statement serving a stranger, a renter and a host at once, with an `is_party`
/// helper cutting the renter, the amount and the hold expiry out of the rows afterwards.
/// The third arm is the one worth understanding: `status IN ('reserved','confirmed')`
/// makes the rows that *block a slot* readable by anyone, because availability is a query
/// over these rows and a prospective renter has to see that a slot is taken.
///
/// | function | namespace | scope |
/// |---|---|---|
/// | [`find_for_public_spot`] | `public` | none — reserved/confirmed rows block a slot for everyone |
/// | [`find_for_host_spot`] | `host` | the parent read already matched `host_id = caller` |
/// | [`find_list_for_renter`] | `renter` | `renter_id = caller` |
/// | [`find_next_for_renter`] | `renter` | `renter_id = caller` |
/// | [`find_for_renter`] | `renter` | `renter_id = caller` |
///
/// No read branches on a caller and nothing is cut after the fact.
///
/// **`/renter/bookings/{id}` is where the old disjunction actually split.** It was
/// `(renter_id = $2 OR host_id = $2)` — a booking has two parties and either could read
/// it by id. Now the renter reads it here and a host reads the same booking as a child of
/// their own spot, which is where a host is already looking. A renter asking for someone
/// else's booking matches no row and gets 404.
///
/// Trade-off, unchanged and stated so it is not rediscovered: booking ids, their slots,
/// their `ends_at` and reserved-vs-confirmed are enumerable per spot. Renter identity,
/// amounts and hold expiry are not — they are in a different projection, selected by a
/// different statement.
///
/// `the_read_model_scopes_what_each_caller_can_see` in this module's `mod.rs` asserts
/// each of these separately.
///
/// [`find_for_public_spot`]: ViewBookingRepository::find_for_public_spot
/// [`find_for_host_spot`]: ViewBookingRepository::find_for_host_spot
/// [`find_list_for_renter`]: ViewBookingRepository::find_list_for_renter
/// [`find_next_for_renter`]: ViewBookingRepository::find_next_for_renter
/// [`find_for_renter`]: ViewBookingRepository::find_for_renter
pub struct ViewBookingRepository;

impl ViewBookingRepository {
    /// **The availability answer** for one spot, as of `now`: what still blocks a slot.
    ///
    /// Five columns, no join, no identity. The scoped half is not cut here — it is never
    /// selected, so there is no row in flight carrying a renter's name to a reader who may
    /// not have it.
    ///
    /// `belonging_to` rather than `spot_id.eq(id)`: the parent row is already in hand, so
    /// the foreign key is read off it instead of being passed again. `status IN
    /// ('reserved','confirmed')` is what makes these rows public, and also why no client
    /// has to remember to exclude released and cancelled bookings. `ends_at > $2` bounds
    /// it, served by `booking_spot (spot_id, ends_at)`.
    pub async fn find_for_public_spot(
        conn: &mut AsyncPgConnection,
        spot: &PublicSpotProjection,
        now: DateTime<Utc>,
    ) -> MyResult<Vec<PublicBookingProjection>> {
        Ok(PublicBookingProjection::belonging_to(spot)
            .filter(
                booking::ends_at
                    .gt(now)
                    .and(booking::status.eq_any(["reserved", "confirmed"])),
            )
            .order(booking::ends_at.asc())
            .select(PublicBookingProjection::as_select())
            .load(conn)
            .await?)
    }

    /// Every booking still to come on one spot, in full, for its host.
    ///
    /// **No caller and no status filter.** Both would be redundant: the `spot` this
    /// belongs to came back from `ViewSpotRepository::find_for_host`, whose
    /// `host_id = $2` already matched, and a host is entitled to their own released and
    /// cancelled rows — which is exactly the history the manage screen shows.
    ///
    /// The renter join resolves to `None` for a renter this service has not projected
    /// yet.
    pub async fn find_for_host_spot(
        conn: &mut AsyncPgConnection,
        spot: &HostSpotProjection,
        now: DateTime<Utc>,
    ) -> MyResult<Vec<HostBookingProjection>> {
        Ok(HostBookingProjection::belonging_to(spot)
            .left_join(app_user::table.on(app_user::id.eq(booking::renter_id)))
            .filter(booking::ends_at.gt(now))
            .order(booking::ends_at.asc())
            .select(HostBookingProjection::as_select())
            .load(conn)
            .await?)
    }

    /// The caller's own bookings as a renter, newest first.
    ///
    /// Scoped by `renter_id = $1` rather than by a select rule: this endpoint answers
    /// "mine", and a booking the caller merely *hosts* belongs on the spot's page instead.
    /// Nothing here needs cutting, because everything returned is theirs.
    ///
    /// `RenterBookingProjection::query()` is the `booking ⟕ spot` join, held by
    /// `HasQuery` because all three renter reads use the same one. The scope is still a
    /// `.filter()` — `base_query` takes no arguments, so it cannot hold an identity.
    pub async fn find_list_for_renter(
        conn: &mut AsyncPgConnection,
        renter_id: Uuid,
    ) -> MyResult<Vec<RenterBookingProjection>> {
        Ok(RenterBookingProjection::query()
            .filter(booking::renter_id.eq(renter_id))
            .order(booking::ends_at.desc())
            .load(conn)
            .await?)
    }

    /// The caller's soonest booking that has not ended, or `None`.
    ///
    /// **This is one row, and it used to be the whole history.** The home screen fetched
    /// every booking the renter had ever made, filtered to `confirmed` client-side, and
    /// ranked what was left by comparing `"YYYY-MM-DDTHH:MM"` wall-clock strings — which
    /// orders wrong across time zones, as that file's own comment said.
    ///
    /// `ends_at` is an instant, so `ORDER BY ends_at LIMIT 1` is correct in every zone at
    /// once, and `booking_renter_ends (renter_id, ends_at)` makes it a one-row seek.
    ///
    /// `confirmed` only: a `reserved` hold is an unfinished checkout, not somewhere the
    /// renter is due.
    pub async fn find_next_for_renter(
        conn: &mut AsyncPgConnection,
        renter_id: Uuid,
        now: DateTime<Utc>,
    ) -> MyResult<Option<RenterBookingProjection>> {
        Ok(RenterBookingProjection::query()
            .filter(
                booking::renter_id
                    .eq(renter_id)
                    .and(booking::status.eq("confirmed"))
                    .and(booking::ends_at.gt(now)),
            )
            .order(booking::ends_at.asc())
            .first(conn)
            .await
            .optional()?)
    }

    /// One of the caller's own bookings, by id.
    ///
    /// `renter_id = $2` and not the old `(renter_id = $2 OR host_id = $2)`: a host reads
    /// the same booking as a child of their own spot, under `/host/spots/{id}`. Anyone
    /// else matches no row and the route answers 404.
    ///
    /// That is a deliberate tightening on the public side too. A stranger used to reach a
    /// `reserved` or `confirmed` booking by id and receive its four public fields. Nothing
    /// needed that: availability is answered by the spot's own read, which is where a
    /// prospective renter is already looking.
    pub async fn find_for_renter(
        conn: &mut AsyncPgConnection,
        booking_id: Uuid,
        renter_id: Uuid,
    ) -> MyResult<Option<RenterBookingProjection>> {
        Ok(RenterBookingProjection::query()
            .filter(
                booking::id
                    .eq(booking_id)
                    .and(booking::renter_id.eq(renter_id)),
            )
            .first(conn)
            .await
            .optional()?)
    }

    /// Insert-or-replace the whole row.
    ///
    /// One statement, where this used to be two. A `CONTENT $row` write cleared the
    /// `spot` and `renter` record links, so `link_refs` had to put them back in the
    /// same transaction — a sequence that was correct only if both halves ran, which
    /// no amount of Rust could show and a live test had to check against a real
    /// database.
    ///
    /// `spot_id` and `renter_id` are plain uuid columns written by this statement like
    /// any other, so there is no second half and nothing to prove about it.
    pub async fn upsert(conn: &mut AsyncPgConnection, booking: ViewBooking) -> MyResult<()> {
        diesel::insert_into(booking::table)
            .values(booking.clone())
            .on_conflict(booking::id)
            .do_update()
            .set(booking)
            .execute(conn)
            .await?;
        Ok(())
    }

    /// Settles a booking, but only out of the status the event is allowed to leave.
    ///
    /// The `WHERE` is the whole guard: a payment landing microseconds before a hold
    /// lapses, with the sweeper's expiry arriving second, must not undo the
    /// confirmation. And `hold_until = NULL` is a *clear*, which a patch's `COALESCE`
    /// can express only as "leave alone" — so it is assigned unconditionally and is
    /// deliberately not a field on [`ViewBookingPatch`], which means the two cannot
    /// fight over it.
    ///
    /// `from` is a parameter because a cancel leaves `confirmed`, not `reserved`, and
    /// hardcoding one would drop the other **silently**. Matching nothing is a
    /// legitimate no-op, not an error — this row *is* the availability answer, so no
    /// second copy needs recomputing either way.
    pub async fn settle(
        conn: &mut AsyncPgConnection,
        booking_id: Uuid,
        from: &str,
        patch: ViewBookingPatch,
    ) -> MyResult<()> {
        // `AsChangeset` skips an absent field, which is the `COALESCE` this replaces —
        // but it would skip `hold_until` too, and that one is a *clear*. It is assigned
        // beside the patch rather than through it, for the same reason it is not a field
        // on `ViewBookingPatch`.
        diesel::update(
            booking::table.filter(booking::id.eq(booking_id).and(booking::status.eq(from))),
        )
        .set((&patch, booking::hold_until.eq(None::<DateTime<Utc>>)))
        .execute(conn)
        .await?;
        Ok(())
    }
}

// `link_refs` is gone with the `spot` and `renter` record links it wrote.
//
// It is worth recording what it had already survived, because the shape of the
// problem is what the uuid columns remove. It was originally a subquery —
// `SELECT … WHERE record::id(id) = $x` — which resolved to NONE whenever the target
// had not been projected yet and left the link null *permanently*, since nothing
// revisited the row. That was patched with `backfill_links` methods that ran when the
// target arrived, which covered the sequential cases and not the concurrent one: USERS
// and BOOKINGS advance independently, so each side could start its transaction before
// the other's row was visible, neither resolve, and the link stay null for good.
//
// The fix then was to write the link unconditionally and let it resolve itself. The
// fix now is that there is no link: a read LEFT JOINs on the uuid, and a reference to
// a row that has not arrived is simply an absent join.
