use bus::Projector;
use chrono::{DateTime, Utc};
use diesel_async::AsyncPgConnection;
use shared::{
    domain_models::{
        booking::status as booking_status,
        payment::{BookingMirror, BookingMirrorPatch},
    },
    error::myerror::MyResult,
    events::{STREAM_BOOKINGS, STREAM_USERS, booking::BookingEvent, user::UserEvent},
};

use crate::repository::{
    booking_mirror_repository::BookingMirrorRepository,
    host_mirror_repository::HostMirrorRepository,
};

/// payment-service consumes BOOKINGS as well as its own stream, for two things it
/// cannot answer otherwise: what a booking costs and who it belongs to (before a
/// payment may be created), and whether it is still going to happen (before money is
/// kept or returned).
///
/// Read-only as far as the outside world is concerned. The refund side effect that
/// *reacts* to these same events is a `Worker`, not this projector — see worker.rs for
/// why that distinction is not optional.
pub struct BookingProjector;

impl Projector for BookingProjector {
    const STREAM: &'static str = STREAM_BOOKINGS;
    /// Not `payment-bookings` — that is [`crate::worker::BookingWorker`]'s, and this
    /// is the one place in the codebase where a projector and a worker read the same
    /// stream. Two consumers, deliberately: the projector delivers from sequence 1 to
    /// every replica, the worker delivers new messages to one. Sharing a name asks
    /// JetStream for both at once, and it refuses — "deliver policy can not be
    /// updated" — leaving whichever lost the race permanently stalled.
    const DURABLE: &'static str = "payment-booking-mirror";
    type Event = BookingEvent;

    async fn apply(
        &self,
        conn: &mut AsyncPgConnection,
        event: BookingEvent,
        _at: DateTime<Utc>,
        version: i64,
    ) -> MyResult<()> {
        let booking_id = event.booking_id();

        match event {
            // A whole-row write, not a merge: BookingCreated is always the first event
            // for a booking and BOOKINGS never expires, so this row is only ever
            // created complete — which is also why nothing on this table is `Option`
            // except the genuinely optional columns.
            BookingEvent::Created(e) => {
                BookingMirrorRepository::upsert(&mut *conn, BookingMirror::created(e)).await
            }

            BookingEvent::Confirmed { booking_id } => {
                BookingMirrorRepository::transition(
                    &mut *conn,
                    booking_id,
                    &[booking_status::RESERVED],
                    BookingMirrorPatch::status(booking_status::CONFIRMED),
                )
                .await
            }

            BookingEvent::Released { booking_id, reason } => {
                BookingMirrorRepository::transition(
                    &mut *conn,
                    booking_id,
                    &[booking_status::RESERVED],
                    BookingMirrorPatch::released(reason),
                )
                .await
            }

            BookingEvent::Cancelled { booking_id, reason } => {
                BookingMirrorRepository::transition(
                    &mut *conn,
                    booking_id,
                    &[booking_status::CONFIRMED],
                    BookingMirrorPatch::cancelled(reason),
                )
                .await
            }
        }?;

        shared::set_version!(conn, "booking", shared::schema::payment::booking, &booking_id, version)
    }
}

/// USERS, into the `host` mirror: the email and country Stripe demands before it will
/// open a connected account.
///
/// A second foreign stream, and it is here for the reason `shared::rpc` gives for not
/// being here — a value onboarding cannot proceed without must not depend on another
/// service answering a request. See `migrations/payment/0004_host_mirror/up.sql`.
///
/// Only two of the five USERS variants matter. A password change, an email
/// verification and a resend request change nothing Stripe is ever told.
pub struct UserProjector;

impl Projector for UserProjector {
    const STREAM: &'static str = STREAM_USERS;
    const DURABLE: &'static str = "payment-host-mirror";
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
                HostMirrorRepository::upsert(&mut *conn, &user_id, &e.email).await
            }

            // `None` is unchanged in the event and unchanged in the write — the same
            // rule the profile form sends. A country arrives only this way: it is not
            // asked for at signup.
            UserEvent::Updated(e) => {
                HostMirrorRepository::patch(&mut *conn, &user_id, e.email, e.country).await
            }

            // Deliberately ignored, and each for its own reason: a password hash must
            // never reach this database, `EmailVerified` gates login rather than
            // payouts, and `VerificationRequested` is notification-service's alone.
            _ => Ok(()),
        }?;

        // `host`, not `app_user`. This line used to say `set_version(conn, "user", …)`,
        // which resolved through an allowlist to `app_user` — a table this database does
        // not have — so a runtime catalogue check turned it into a silent no-op on every
        // USERS event. The typed table is what surfaced that: `shared::schema::payment`
        // declares no `app_user`, so the old spelling no longer compiles.
        //
        // The aggregate stays `"user"` because that is the protocol name — what the log
        // line and a client's `user:<id>@N` version spell — while `host` is where this
        // service happens to keep it. They differ here exactly as `user`/`app_user` do
        // everywhere else.
        shared::set_version!(conn, "user", shared::schema::payment::host, &user_id, version)
    }
}

// `PaymentProjector` is gone. This service's own PAYMENTS events are no longer
// projected back in: `payment_service` and `settlement_worker_service` write the
// `payment` and `payout` rows directly, inside the transaction that enqueues the
// event. Consuming its own stream would have re-applied writes it had already
// made.
//
// What remains is the BOOKINGS mirror above — a *foreign* stream, which is exactly
// what a projector is still for.
