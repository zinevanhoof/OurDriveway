use bus::Projector;
use chrono::{DateTime, Utc};
use shared::db;
use shared::{
    domain_models::{
        booking::status as booking_status,
        view::{
            booking::{ViewBooking, ViewBookingPatch},
            payout::ViewPayout,
            spot::ViewSpotPatch,
            user::{ViewUser, ViewUserPatch},
        },
    },
    error::myerror::MyResult,
    events::{
        STREAM_BOOKINGS, STREAM_PAYMENTS, STREAM_SPOTS, STREAM_USERS, booking::BookingEvent,
        payment::PaymentEvent, spot::SpotEvent, user::UserEvent,
    },
};
use sqlx::PgConnection;

use crate::repository::{
    booking_repository::ViewBookingRepository, payout_repository::ViewPayoutRepository,
    spot_repository::ViewSpotRepository, user_repository::ViewUserRepository,
};

/// One projector per stream, each with its own consumer. They advance independently,
/// which is exactly why a spot can be applied before the user it references — hence
/// the link resolution and backfill throughout `crate::repository`.
///
/// None of them holds a connection: `bus::Tx` hands each a `&Transaction` per event
/// and advances that stream's cursor inside it. Every method below used to end with a
/// hand-written `UPSERT _projection:… SET last_seq` inside its own `BEGIN`/`COMMIT`;
/// forgetting one meant an event that replayed forever.
pub struct UserProjector;

impl Projector for UserProjector {
    const STREAM: &'static str = STREAM_USERS;
    const DURABLE: &'static str = "view-users";
    type Event = UserEvent;

    async fn apply(
        &self,
        conn: &mut PgConnection,
        event: UserEvent,
        _at: DateTime<Utc>,
        version: u64,
    ) -> MyResult<()> {
        let user_id = event.user_id();

        match event {
            UserEvent::Registered(e) => {
                // A whole-row write, with nothing to relink afterwards. Every
                // reference into this table is a plain uuid resolved by a LEFT JOIN at
                // read time, so a row that pointed here before this user existed is
                // already correct.
                ViewUserRepository::upsert(&mut *conn, ViewUser::registered(e, version)).await
            }

            UserEvent::Updated(e) => {
                ViewUserRepository::patch(&mut *conn, user_id, ViewUserPatch::from(e)).await
            }

            // Nothing to project — the hash never comes near this database.
            UserEvent::PasswordChanged(_) => Ok(()),

            // Verification state is an authentication concern and stays in
            // user-service's private projection. This table is world-readable, so
            // `email_verified` here would publish which addresses are unconfirmed to
            // every client that can read a spot owner's profile.
            UserEvent::EmailVerified { .. } | UserEvent::VerificationRequested(_) => Ok(()),
        }?;

        // After the match, so it runs for the arms that store nothing too. "Applied"
        // means *seen and decided about*, not *changed a column* — a version that
        // only advanced on writes would strand a client waiting on `user:<id>@2`
        // after an `EmailVerified` this table deliberately ignores.
        db::set_version(conn, "user", &user_id, version).await
    }
}

pub struct SpotProjector;

impl Projector for SpotProjector {
    const STREAM: &'static str = STREAM_SPOTS;
    const DURABLE: &'static str = "view-spots";
    type Event = SpotEvent;

    async fn apply(
        &self,
        conn: &mut PgConnection,
        event: SpotEvent,
        at: DateTime<Utc>,
        version: u64,
    ) -> MyResult<()> {
        match event {
            SpotEvent::Created(e) => {
                let spot_id = e.spot_id;
                // One statement. `owner_id` rides in the patch like any other column;
                // there is no `owner` link left for a second statement to restore.
                ViewSpotRepository::merge(&mut *conn, spot_id, ViewSpotPatch::created(e, at))
                    .await?;
                db::set_version(conn, "spot", &spot_id, version).await
            }

            // `patch`, not `merge`, for the two below: only `Created` may bring a spot
            // row into existence. These arrive after it on the same ordered stream, so
            // a missing row means something is already wrong — and `UPDATE` matching
            // nothing is a safer answer than an upsert building a partial row that the
            // NOT NULL columns cannot satisfy.
            SpotEvent::Updated(e) => {
                let spot_id = e.spot_id;
                ViewSpotRepository::patch(&mut *conn, spot_id, ViewSpotPatch::updated(e, at))
                    .await?;
                db::set_version(conn, "spot", &spot_id, version).await
            }

            // Soft delete. The row stays selectable so a renter's past booking still
            // resolves a title and an address — the lists filter `deleted`.
            SpotEvent::Deleted { spot_id } => {
                ViewSpotRepository::patch(&mut *conn, spot_id, ViewSpotPatch::deleted(at))
                    .await?;
                db::set_version(conn, "spot", &spot_id, version).await
            }
        }
    }
}

pub struct BookingProjector;

impl Projector for BookingProjector {
    const STREAM: &'static str = STREAM_BOOKINGS;
    const DURABLE: &'static str = "view-bookings";
    type Event = BookingEvent;

    async fn apply(
        &self,
        conn: &mut PgConnection,
        event: BookingEvent,
        at: DateTime<Utc>,
        version: u64,
    ) -> MyResult<()> {
        // No spot to touch afterwards. Availability is a query over these rows, so
        // writing one is the whole of applying the event — nothing derived has to be
        // recomputed and kept in step.
        match event {
            BookingEvent::Created(e) => {
                let booking_id = e.booking_id;
                // One statement. `spot_id` and `renter_id` ride in the row; there are
                // no links for a second statement to restore.
                ViewBookingRepository::upsert(
                    &mut *conn,
                    ViewBooking::created(e, at, version),
                )
                .await?;
                db::set_version(conn, "booking", &booking_id, version).await
            }

            BookingEvent::Confirmed { booking_id } => {
                ViewBookingRepository::settle(
                    &mut *conn,
                    booking_id,
                    booking_status::RESERVED,
                    ViewBookingPatch::confirmed(),
                )
                .await?;
                db::set_version(conn, "booking", &booking_id, version).await
            }

            BookingEvent::Released { booking_id, reason } => {
                ViewBookingRepository::settle(
                    &mut *conn,
                    booking_id,
                    booking_status::RESERVED,
                    ViewBookingPatch::released(reason),
                )
                .await?;
                db::set_version(conn, "booking", &booking_id, version).await
            }

            BookingEvent::Cancelled { booking_id, reason } => {
                ViewBookingRepository::settle(
                    &mut *conn,
                    booking_id,
                    booking_status::CONFIRMED,
                    ViewBookingPatch::cancelled(reason),
                )
                .await?;
                db::set_version(conn, "booking", &booking_id, version).await
            }
        }
    }
}

/// PAYMENTS, for **payout history only**.
///
/// Deliberately not the payments themselves. A renter's charges and a host's income
/// figures are served by payment-service, which owns them — projecting them here too
/// would give the payout button one answer and the balance next to it another. What
/// belongs in the read model is the *list* of withdrawals, so it can be queried
/// alongside the rest of a profile like everything else.
pub struct PaymentProjector;

impl Projector for PaymentProjector {
    const STREAM: &'static str = STREAM_PAYMENTS;
    const DURABLE: &'static str = "view-payments";
    type Event = PaymentEvent;

    async fn apply(
        &self,
        conn: &mut PgConnection,
        event: PaymentEvent,
        _at: DateTime<Utc>,
        version: u64,
    ) -> MyResult<()> {
        let PaymentEvent::PayoutRequested {
            payout_id,
            owner_id,
            amount_cents,
            requested_at,
        } = event
        else {
            // Every other variant stores nothing. Not a gap: what a renter was
            // charged is payment-service's to answer, and duplicating it here would
            // create a second version of the same money. The cursor still advances,
            // which `Tx` does after this returns.
            return Ok(());
        };

        ViewPayoutRepository::upsert(
            &mut *conn,
            ViewPayout {
                id: payout_id,
                version,
                owner_id,
                amount: amount_cents,
                // The requester's timestamp off the event, not this replica's clock.
                created_at: requested_at,
            },
        )
        .await?;
        // `payout`, not `payment`: this database has no payment table, which is why
        // the version write lives in each projector rather than in `bus::Tx` — a
        // statement naming a table that does not exist is an error, not a no-op.
        db::set_version(conn, "payout", &payout_id, version).await
    }
}
