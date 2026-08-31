use chrono::{DateTime, Utc};
use shared::domain_models::view::booking::{ViewBooking, ViewBookingPatch};
use shared::error::myerror::MyResult;
use shared::projections::booking::{BookingListItem, OwnerViewBooking, PublicViewBooking};
use sqlx::PgExecutor;
use uuid::Uuid;

/// The `booking` table in the read model.
///
/// **What used to be one clause is now two functions.** Every read of this table
/// carried the same disjunction:
///
/// ```sql
/// (b.renter_id = $1 OR b.owner_id = $1 OR b.status IN ('reserved', 'confirmed'))
/// ```
///
/// — one statement serving a stranger and a party at once, with an `is_party` helper
/// cutting the renter, the amount and the hold expiry out of the rows afterwards. The
/// third arm is the one worth understanding: `status IN ('reserved','confirmed')` makes
/// the rows that *block a slot* readable by anyone, because availability is a query over
/// these rows and a prospective renter has to see that a slot is taken.
///
/// That arm is now [`find_all_public_by_spot_id`], which selects four columns and no
/// identity. The other two are [`find_all_owner_by_spot_id`] and [`find_owner_by_id`],
/// which return everything because reaching them already proved who is asking. No read
/// branches on a caller, and nothing is cut after the fact.
///
/// Trade-off, unchanged and stated so it is not rediscovered: booking ids, their slots,
/// their `ends_at` and reserved-vs-confirmed are enumerable per spot. Renter identity,
/// amounts and hold expiry are not — they are in a different projection, selected by a
/// different statement.
///
/// `the_read_model_scopes_what_each_caller_can_see` in this module's `mod.rs` asserts
/// each of these separately.
///
/// [`find_all_public_by_spot_id`]: ViewBookingRepository::find_all_public_by_spot_id
/// [`find_all_owner_by_spot_id`]: ViewBookingRepository::find_all_owner_by_spot_id
/// [`find_owner_by_id`]: ViewBookingRepository::find_owner_by_id
pub struct ViewBookingRepository;

impl ViewBookingRepository {
    /// **The availability answer** for one spot, as of `now`: what still blocks a slot.
    ///
    /// Four columns, no join, no identity. The scoped half is not cut here — it is never
    /// selected, so there is no row in flight carrying a renter's name to a reader who
    /// may not have it.
    ///
    /// `status IN ('reserved','confirmed')` is what makes these rows public, and it is
    /// also why no client has to remember to exclude released and cancelled bookings.
    /// `ends_at > $2` bounds it, served by `booking_spot (spot_id, ends_at)`.
    pub async fn find_all_public_by_spot_id(
        ex: impl PgExecutor<'_>,
        spot_id: Uuid,
        now: DateTime<Utc>,
    ) -> MyResult<Vec<PublicViewBooking>> {
        Ok(sqlx::query_as(
            "SELECT b.id, b.booked, b.status, b.ends_at
               FROM booking b
              WHERE b.spot_id = $1 AND b.ends_at > $2
                AND b.status IN ('reserved', 'confirmed')
              ORDER BY b.ends_at",
        )
        .bind(spot_id)
        .bind(now)
        .fetch_all(ex)
        .await?)
    }

    /// Every booking still to come on one spot, in full, for its host.
    ///
    /// **No caller and no status filter.** Both would be redundant: the only way here is
    /// through `ViewSpotRepository::find_owner_by_id`, whose `owner_id = $2` already
    /// matched, and a host is entitled to their own released and cancelled rows — which
    /// is exactly the history the manage screen shows.
    ///
    /// The renter join resolves to `None` for a renter this service has not projected
    /// yet; see `MaybeJoined`.
    pub async fn find_all_owner_by_spot_id(
        ex: impl PgExecutor<'_>,
        spot_id: Uuid,
        now: DateTime<Utc>,
    ) -> MyResult<Vec<OwnerViewBooking>> {
        Ok(sqlx::query_as(
            "SELECT b.id, b.booked, b.status, b.ends_at,
                    b.spot_id, b.renter_id, b.owner_id, b.amount, b.hold_until,
                    b.release_reason, b.cancel_reason, b.rating, b.created_at,
                    u.id              AS user_id,
                    u.first_name      AS user_first_name,
                    u.last_name       AS user_last_name,
                    u.profile_picture AS user_profile_picture
               FROM booking b
               LEFT JOIN app_user u ON u.id = b.renter_id
              WHERE b.spot_id = $1 AND b.ends_at > $2
              ORDER BY b.ends_at",
        )
        .bind(spot_id)
        .bind(now)
        .fetch_all(ex)
        .await?)
    }

    /// The caller's own bookings as a renter, newest first.
    ///
    /// Scoped by `renter_id = $1` rather than by the select rule: this endpoint answers
    /// "mine", and a booking the caller merely *hosts* belongs on the spot's page
    /// instead. Nothing here needs cutting, because everything returned is theirs.
    pub async fn find_all_by_renter_id(
        ex: impl PgExecutor<'_>,
        renter_id: Uuid,
    ) -> MyResult<Vec<BookingListItem>> {
        Ok(sqlx::query_as(
            "SELECT b.id, b.status, b.amount, b.booked, b.ends_at, b.cancel_reason,
                    b.spot_id,
                    s.title    AS spot_title,
                    s.images   AS spot_images,
                    s.timezone AS spot_timezone,
                    s.address  AS spot_address
               FROM booking b
               LEFT JOIN spot s ON s.id = b.spot_id
              WHERE b.renter_id = $1
              ORDER BY b.ends_at DESC",
        )
        .bind(renter_id)
        .fetch_all(ex)
        .await?)
    }

    /// One booking, whole, for a party to it.
    ///
    /// `(renter_id = $2 OR owner_id = $2)` is the only surviving disjunction in this
    /// file, and it is a genuine one: a booking has two parties and either may read it.
    /// It is not the old mixed-audience clause — a stranger matches no row and the route
    /// answers 404.
    ///
    /// That is a deliberate tightening. A stranger used to reach a `reserved` or
    /// `confirmed` booking by id and receive its four public fields. Nothing needed
    /// that: availability is answered by the spot's own read, which is where a
    /// prospective renter is already looking.
    pub async fn find_owner_by_id(
        ex: impl PgExecutor<'_>,
        booking_id: Uuid,
        caller: Uuid,
    ) -> MyResult<Option<OwnerViewBooking>> {
        Ok(sqlx::query_as(
            "SELECT b.id, b.booked, b.status, b.ends_at,
                    b.spot_id, b.renter_id, b.owner_id, b.amount, b.hold_until,
                    b.release_reason, b.cancel_reason, b.rating, b.created_at,
                    u.id              AS user_id,
                    u.first_name      AS user_first_name,
                    u.last_name       AS user_last_name,
                    u.profile_picture AS user_profile_picture
               FROM booking b
               LEFT JOIN app_user u ON u.id = b.renter_id
              WHERE b.id = $1 AND (b.renter_id = $2 OR b.owner_id = $2)",
        )
        .bind(booking_id)
        .bind(caller)
        .fetch_optional(ex)
        .await?)
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
    pub async fn upsert(ex: impl PgExecutor<'_>, booking: ViewBooking) -> MyResult<()> {
        sqlx::query(
            "INSERT INTO booking
                 (id, version, spot_id, owner_id, renter_id, booked, amount, status,
                  hold_until, release_reason, cancel_reason, rating, ends_at, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
             ON CONFLICT (id) DO UPDATE SET
                 version        = EXCLUDED.version,
                 spot_id        = EXCLUDED.spot_id,
                 owner_id       = EXCLUDED.owner_id,
                 renter_id      = EXCLUDED.renter_id,
                 booked         = EXCLUDED.booked,
                 amount         = EXCLUDED.amount,
                 status         = EXCLUDED.status,
                 hold_until     = EXCLUDED.hold_until,
                 release_reason = EXCLUDED.release_reason,
                 cancel_reason  = EXCLUDED.cancel_reason,
                 rating         = EXCLUDED.rating,
                 ends_at        = EXCLUDED.ends_at,
                 created_at     = EXCLUDED.created_at",
        )
        .bind(booking.id)
        .bind(booking.version as i64)
        .bind(booking.spot_id)
        .bind(booking.owner_id)
        .bind(booking.renter_id)
        .bind(sqlx::types::Json(booking.booked))
        .bind(booking.amount)
        .bind(booking.status)
        .bind(booking.hold_until)
        .bind(booking.release_reason)
        .bind(booking.cancel_reason)
        .bind(booking.rating)
        .bind(booking.ends_at)
        .bind(booking.created_at)
        .execute(ex)
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
        ex: impl PgExecutor<'_>,
        booking_id: Uuid,
        from: &str,
        patch: ViewBookingPatch,
    ) -> MyResult<()> {
        sqlx::query(
            "UPDATE booking SET
                 status         = COALESCE($3, status),
                 release_reason = COALESCE($4, release_reason),
                 cancel_reason  = COALESCE($5, cancel_reason),
                 hold_until     = NULL
             WHERE id = $1 AND status = $2",
        )
        .bind(booking_id)
        .bind(from)
        .bind(patch.status)
        .bind(patch.release_reason)
        .bind(patch.cancel_reason)
        .execute(ex)
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
