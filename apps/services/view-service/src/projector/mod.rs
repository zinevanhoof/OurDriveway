use bus::Projector;
use chrono::{DateTime, Utc};
use diesel_async::AsyncPgConnection;
use shared::{
    domain_models::{
        booking::status as booking_status,
        // The write side's statuses, mirrored rather than re-spelled — this projection
        // is not allowed to invent a fourth one.
        payment::payout::status as payout_status,
        view::{
            booking::{ViewBooking, ViewBookingPatch},
            payment::{ViewPayment, ViewPaymentPatch},
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

use crate::repository::{
    booking_repository::ViewBookingRepository, payment_repository::ViewPaymentRepository,
    payout_repository::ViewPayoutRepository, spot_repository::ViewSpotRepository,
    user_repository::ViewUserRepository,
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
        conn: &mut AsyncPgConnection,
        event: UserEvent,
        _at: DateTime<Utc>,
        version: i64,
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
            // every client that can read a spot host's profile.
            UserEvent::EmailVerified { .. } | UserEvent::VerificationRequested(_) => Ok(()),
        }?;

        // After the match, so it runs for the arms that store nothing too. "Applied"
        // means *seen and decided about*, not *changed a column* — a version that
        // only advanced on writes would strand a client waiting on `user:<id>@2`
        // after an `EmailVerified` this table deliberately ignores.
        shared::set_version!(conn, "user", shared::schema::view::app_user, &user_id, version)
    }
}

pub struct SpotProjector;

impl Projector for SpotProjector {
    const STREAM: &'static str = STREAM_SPOTS;
    const DURABLE: &'static str = "view-spots";
    type Event = SpotEvent;

    async fn apply(
        &self,
        conn: &mut AsyncPgConnection,
        event: SpotEvent,
        at: DateTime<Utc>,
        version: i64,
    ) -> MyResult<()> {
        match event {
            SpotEvent::Created(e) => {
                let spot_id = e.spot_id;
                // One statement. `host_id` rides in the patch like any other column;
                // there is no `host` link left for a second statement to restore.
                ViewSpotRepository::merge(&mut *conn, spot_id, ViewSpotPatch::created(e, at))
                    .await?;
                shared::set_version!(conn, "spot", shared::schema::view::spot, &spot_id, version)
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
                shared::set_version!(conn, "spot", shared::schema::view::spot, &spot_id, version)
            }

            // Soft delete. The row stays selectable so a renter's past booking still
            // resolves a title and an address — the lists filter `deleted`.
            SpotEvent::Deleted { spot_id } => {
                ViewSpotRepository::patch(&mut *conn, spot_id, ViewSpotPatch::deleted(at)).await?;
                shared::set_version!(conn, "spot", shared::schema::view::spot, &spot_id, version)
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
        conn: &mut AsyncPgConnection,
        event: BookingEvent,
        at: DateTime<Utc>,
        version: i64,
    ) -> MyResult<()> {
        // No spot to touch afterwards. Availability is a query over these rows, so
        // writing one is the whole of applying the event — nothing derived has to be
        // recomputed and kept in step.
        match event {
            BookingEvent::Created(e) => {
                let booking_id = e.booking_id;
                // One statement. `spot_id` and `renter_id` ride in the row; there are
                // no links for a second statement to restore.
                ViewBookingRepository::upsert(&mut *conn, ViewBooking::created(e, at, version))
                    .await?;
                shared::set_version!(conn, "booking", shared::schema::view::booking, &booking_id, version)
            }

            BookingEvent::Confirmed { booking_id } => {
                ViewBookingRepository::settle(
                    &mut *conn,
                    booking_id,
                    booking_status::RESERVED,
                    ViewBookingPatch::confirmed(),
                )
                .await?;
                shared::set_version!(conn, "booking", shared::schema::view::booking, &booking_id, version)
            }

            BookingEvent::Released { booking_id, reason } => {
                ViewBookingRepository::settle(
                    &mut *conn,
                    booking_id,
                    booking_status::RESERVED,
                    ViewBookingPatch::released(reason),
                )
                .await?;
                shared::set_version!(conn, "booking", shared::schema::view::booking, &booking_id, version)
            }

            BookingEvent::Cancelled { booking_id, reason } => {
                ViewBookingRepository::settle(
                    &mut *conn,
                    booking_id,
                    booking_status::CONFIRMED,
                    ViewBookingPatch::cancelled(reason),
                )
                .await?;
                shared::set_version!(conn, "booking", shared::schema::view::booking, &booking_id, version)
            }
        }
    }
}

/// PAYMENTS, into two tables: `payment` and `payout`.
///
/// **This used to project payouts and nothing else**, on the argument that a second
/// copy of a renter's charges would disagree with the balance beside the withdraw
/// button. The split is the ordinary one now — payment-service writes, view-service
/// reads — and the wallet needs the charges, so they are here.
///
/// What keeps the old argument from coming true is that no money is ever *spent*
/// against this table. `PaymentService::request_payout` computes what it pays out from
/// payment-service's own rows, inside its own transaction, under an advisory lock; this
/// projection is only ever displayed, and it is allowed to lag by however far the
/// projector is behind. See `migrations/view/0003_payment/up.sql`.
pub struct PaymentProjector;

impl Projector for PaymentProjector {
    const STREAM: &'static str = STREAM_PAYMENTS;
    const DURABLE: &'static str = "view-payments";
    type Event = PaymentEvent;

    async fn apply(
        &self,
        conn: &mut AsyncPgConnection,
        event: PaymentEvent,
        _at: DateTime<Utc>,
        version: i64,
    ) -> MyResult<()> {
        match event {
            // A payout is its own aggregate on this stream — `payout:<uuid>`, a
            // different table and a different version counter from every other variant
            // here, which is why `PaymentEvent::payment_id` answers `None` for it.
            PaymentEvent::PayoutRequested {
                payout_id,
                host_id,
                amount_cents,
                requested_at,
            } => {
                ViewPayoutRepository::upsert(
                    &mut *conn,
                    ViewPayout {
                        id: payout_id,
                        version,
                        host_id,
                        amount: amount_cents,
                        // The transfer has not been attempted yet. This is what the
                        // wallet renders with its PENDING chip.
                        status: payout_status::REQUESTED.to_string(),
                        // The requester's timestamp off the event, not this replica's
                        // clock.
                        created_at: requested_at,
                    },
                )
                .await?;
                shared::set_version!(conn, "payout", shared::schema::view::payout, &payout_id, version)
            }

            // The worker's outcome. One column, and the row is certainly here: these
            // share a subject with the request above, so they share a lane and arrive
            // after it.
            //
            // `transfer_id` and `reason` are dropped on the floor on purpose, exactly
            // as the payment handles are below — a Stripe id and a log message, neither
            // of which a browser has any use for.
            PaymentEvent::PayoutPaid { payout_id, .. } => {
                ViewPayoutRepository::set_status(&mut *conn, payout_id, payout_status::PAID)
                    .await?;
                shared::set_version!(conn, "payout", shared::schema::view::payout, &payout_id, version)
            }

            // A failed withdrawal leaves the row here, marked — the wallet's queries
            // filter it out of both the list and the balance, which is how the money
            // comes back. Deleting it instead would lose the audit and make a
            // redelivered event recreate it as `requested`.
            PaymentEvent::PayoutFailed { payout_id, .. } => {
                ViewPayoutRepository::set_status(&mut *conn, payout_id, payout_status::FAILED)
                    .await?;
                shared::set_version!(conn, "payout", shared::schema::view::payout, &payout_id, version)
            }

            PaymentEvent::Created(e) => {
                let payment_id = e.payment_id;
                ViewPaymentRepository::upsert(&mut *conn, ViewPayment::created(e, version)).await?;
                shared::set_version!(conn, "payment", shared::schema::view::payment, &payment_id, version)
            }

            // The four transitions. Each is one patch and the version write, and the
            // row they patch always exists by the time they arrive: a payment's events
            // share one subject, so they share one projector lane and stay ordered.
            //
            // `intent_id`, `refund_id` and `reason` are dropped on the floor here on
            // purpose — the Stripe handles are the write side's business, and Stripe's
            // failure message is for our log rather than for a screen.
            PaymentEvent::Succeeded { payment_id, .. } => {
                ViewPaymentRepository::transition(
                    &mut *conn,
                    payment_id,
                    ViewPaymentPatch::succeeded(),
                )
                .await?;
                shared::set_version!(conn, "payment", shared::schema::view::payment, &payment_id, version)
            }

            PaymentEvent::Failed { payment_id, .. } => {
                ViewPaymentRepository::transition(
                    &mut *conn,
                    payment_id,
                    ViewPaymentPatch::failed(),
                )
                .await?;
                shared::set_version!(conn, "payment", shared::schema::view::payment, &payment_id, version)
            }

            PaymentEvent::Refunded {
                payment_id,
                refunded_at,
                ..
            } => {
                ViewPaymentRepository::transition(
                    &mut *conn,
                    payment_id,
                    // Off the event, so this replica dates the refund exactly where
                    // payment-service's own row does.
                    ViewPaymentPatch::refunded(refunded_at),
                )
                .await?;
                shared::set_version!(conn, "payment", shared::schema::view::payment, &payment_id, version)
            }

            PaymentEvent::SessionExpired { payment_id, .. } => {
                ViewPaymentRepository::transition(
                    &mut *conn,
                    payment_id,
                    ViewPaymentPatch::expired(),
                )
                .await?;
                shared::set_version!(conn, "payment", shared::schema::view::payment, &payment_id, version)
            }
        }
    }
}
