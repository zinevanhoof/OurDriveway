//! Write side. Validates, writes its own rows, and enqueues the event beside them
//! — all in one transaction.
//!
//! Every payment row this service reads back was written by the request that
//! caused it, not by a projector applying an event a moment later.

use std::sync::Arc;

use async_nats::jetstream::Context;
use bus::outbox;
use chrono::{DateTime, Utc};
use diesel_async::AsyncConnection;
use diesel_async::scoped_futures::ScopedFutureExt;
use shared::db;
use shared::{
    domain_models::{
        booking::status as booking_status,
        payment::{Earnings, Payment, PaymentPatch, Payout, status},
    },
    error::myerror::{ContextExt, MyError, MyResult},
    events::{
        Envelope, aggregate_id, format_version,
        payment::{PaymentCreated, PaymentEvent},
        payment_subject, payout_subject,
    },
    requests::payment::CreateSessionRequest,
    rpc::spot::{SUBJECT_SPOT_CARD, SpotCard},
};
use uuid::Uuid;

use axum::http::StatusCode;

use crate::{
    client::stripe::{NewSession, Outcome, SessionState, Stripe},
    policy::{self, payout::Rejection},
    repository::{
        booking_mirror_repository::BookingMirrorRepository, payment_repository::PaymentRepository,
        payout_repository::PayoutRepository,
    },
};

pub struct PaymentService {
    /// Only for the one thing JetStream is wrong for: asking spot-service what a
    /// spot is called. See [`shared::rpc`].
    ///
    /// There is no `js` here any more — this service publishes nothing directly.
    /// Events go into `_outbox` in the same transaction as the rows, and the relay
    /// carries them.
    nc: async_nats::Client,
    /// The pool. See the note on `UserService::db` — the repositories are stateless,
    /// so this service no longer holds one per table.
    db: shared::db::Db,
    stripe: Arc<Stripe>,
    /// How long after a booking ends its money becomes withdrawable.
    settlement_secs: i64,
}

impl PaymentService {
    pub fn new(js: Context, db: shared::db::Db, stripe: Arc<Stripe>, settlement_secs: i64) -> Self {
        Self {
            nc: js.client().clone(),
            db,
            stripe,
            settlement_secs,
        }
    }

    /// Hands the renter a client secret for the Payment Element.
    ///
    /// Idempotent twice over, and the two layers are not redundant:
    ///
    /// 1. **A payment we already know about is never re-created.** "Continue payment" on a
    ///    reserved booking retrieves the session it already has. Nothing is rebuilt, so
    ///    nothing can be rebuilt *differently* — which matters because the Stripe
    ///    idempotency key is derived from the booking, and Stripe rejects a replay whose
    ///    parameters differ. A spot retitled between reserving and paying would otherwise
    ///    fail the resume, the same way an `expires_at` taken from `Utc::now()` once did.
    /// 2. **Below that, the key still holds.** Two checkouts submitted at once both find
    ///    no payment row — the projector applies our event a moment after this returns —
    ///    so both reach Stripe, and the key is what makes them the same session rather
    ///    than two payable ones.
    ///
    /// Layer 2 is also what makes `payment_booking UNIQUE` in the schema safe.
    pub async fn create_session(
        &self,
        renter_id: &Uuid,
        request: CreateSessionRequest,
    ) -> MyResult<NewSession> {
        let booking_id = &request.booking_id;
        let mut read = db::conn(&self.db).await?;
        let booking = BookingMirrorRepository::find_by_id(&mut read, *booking_id)
            .await?
            .context_not_found(("Not Found", "That booking doesn't exist."))?;

        // Identity comes from the verified token, and this is the only check that
        // stops one renter paying for — and thereby confirming — another's booking.
        (booking.renter_id == *renter_id)
            .context_forbidden(("Forbidden", "That booking isn't yours."))?;

        (booking.status == booking_status::RESERVED)
            .context_conflict(("Conflict", "That booking is no longer awaiting payment."))?;

        // An expired hold must not be payable. The sweeper may not have collected it yet,
        // so the timestamp is the authority here, not the status.
        //
        // Kept rather than just checked: it anchors the session's `expires_at`, which has
        // to be identical on every call for this booking or the idempotent replay behind
        // "Continue payment" is rejected. See SESSION_GRACE_MINUTES.
        let hold_until = booking
            .hold_until
            .filter(|until| *until > Utc::now())
            .context_status(
                StatusCode::GONE,
                (
                    "Hold Expired",
                    "This reservation has expired. Please choose your times again.",
                ),
            )?;

        // Resume: this booking already has a session, so hand back that one. See the
        // note on layer 1 above — re-creating it is not merely wasteful, it is the thing
        // that breaks. A session that has since expired falls through, where the guards
        // above have already refused anything whose hold is gone.
        let mut read = db::conn(&self.db).await?;
        if let Some(payment) = PaymentRepository::find_by_booking_id(&mut read, *booking_id).await?
        {
            let state = self.stripe.retrieve_session(&payment.session_id).await?;
            if let Some(client_secret) = state.client_secret {
                return Ok(NewSession {
                    session_id: payment.session_id,
                    client_secret,
                });
            }
        }

        // What the renter is buying, for the line item. Best-effort on purpose: a spot
        // this cannot describe still gets paid for, it just says less. Nothing below is
        // allowed to depend on it — see the note on `shared::rpc`.
        let spot_id = booking.spot_id;
        let card: Option<SpotCard> = bus::service::request(&self.nc, SUBJECT_SPOT_CARD, &spot_id)
            .await
            .unwrap_or_else(|e| {
                tracing::warn!(spot_id = %spot_id, error = %e, "no spot card; labelling with the booking id");
                None
            });

        // One payment per booking, named after it. Deterministic rather than random so
        // that creating an intent is idempotent without a read: the second attempt
        // upserts the same row instead of tripping the UNIQUE index on `booking_id`, and
        // a webhook can name the payment without looking it up.
        let payment_id = Uuid::new_v5(
            &Uuid::NAMESPACE_OID,
            format!("payment:{booking_id}").as_bytes(),
        );

        let session = self
            .stripe
            .create_session(
                booking_id,
                booking.amount_cents,
                &booking.booked,
                card.as_ref(),
                hold_until,
                &request.return_url,
            )
            .await?;

        let created = PaymentCreated {
            payment_id,
            booking_id: *booking_id,
            host_id: booking.host_id,
            renter_id: booking.renter_id,
            session_id: session.session_id.clone(),
            amount_cents: booking.amount_cents,
            created_at: Utc::now(),
        };

        let mut conn = db::conn(&self.db).await?;

        conn.transaction::<_, MyError, _>(|conn| {
            async move {
                let version = shared::next_version!(conn, shared::schema::payment::payment, &payment_id)?;

                PaymentRepository::upsert(conn, Payment::created(created.clone(), version)).await?;

                // Deterministic event id, so a resubmitted checkout is discarded by the
                // stream's duplicate window instead of appended twice. The `_outbox` row is
                // keyed by it too, so a retry inside this transaction is one row either way.
                let mut envelope = Envelope::new(
                    PaymentEvent::Created(created),
                    None,
                    aggregate_id("payment", &payment_id),
                    version,
                );
                envelope.event_id = Uuid::new_v5(
                    &Uuid::NAMESPACE_OID,
                    format!("payment-created:{payment_id}").as_bytes(),
                );

                outbox::enqueue(conn, &payment_subject(booking_id), &envelope).await?;
                Ok(())
            }
            .scope_boxed()
        })
        .await?;

        Ok(session)
    }

    /// What became of a session, for the checkout screen.
    ///
    /// Authorized against our own `payment` row before Stripe is asked anything, so a
    /// session id belonging to someone else reveals nothing. **404 rather than 403**: a
    /// 403 would confirm the session exists to a caller who cannot see it.
    ///
    /// The status itself comes from Stripe rather than our projection, because our
    /// projection lags the webhook and "not confirmed yet" is indistinguishable from
    /// "failed" from the outside. Stripe knows now.
    pub async fn session_state(
        &self,
        renter_id: &Uuid,
        session_id: &str,
    ) -> MyResult<(SessionState, Uuid)> {
        // The same answer for a session that does not exist and one that is not the
        // caller's — a 403 would confirm it exists to someone who cannot see it.
        const NOT_FOUND: (&str, &str) = ("Not Found", "No such checkout.");

        let mut read = db::conn(&self.db).await?;
        let payment = PaymentRepository::find_by_session_id(&mut read, session_id.to_string())
            .await?
            .context_not_found(NOT_FOUND)?;

        (payment.renter_id == *renter_id).context_not_found(NOT_FOUND)?;

        let state = self.stripe.retrieve_session(session_id).await?;
        Ok((state, payment.booking_id))
    }

    /// Records what a verified webhook said.
    ///
    /// Publishes unconditionally for `Succeeded`. Money has already moved by the time
    /// Stripe tells us, so there is nothing to refuse — a booking that no longer fits
    /// is withdrawn downstream and refunded by `settle_up`, which is one refund trigger
    /// rather than a decision made here with half the information.
    pub async fn record_webhook(&self, outcome: Outcome) -> MyResult<()> {
        let (event, booking_id, dedupe, payment_id) = match outcome {
            Outcome::Succeeded {
                booking_id,
                intent_id,
            } => {
                // Same derivation as `create_session` — that is the point: the webhook
                // names the payment without reading it back.
                let payment_id = Uuid::new_v5(
                    &Uuid::NAMESPACE_OID,
                    format!("payment:{booking_id}").as_bytes(),
                );
                (
                    PaymentEvent::Succeeded {
                        payment_id,
                        booking_id,
                        intent_id,
                    },
                    booking_id,
                    format!("payment-succeeded:{payment_id}"),
                    payment_id,
                )
            }

            Outcome::Failed { booking_id, reason } => {
                let payment_id = Uuid::new_v5(
                    &Uuid::NAMESPACE_OID,
                    format!("payment:{booking_id}").as_bytes(),
                );
                (
                    // Keyed on the reason as well as the payment: a renter retrying a
                    // declined card twice produces two genuine failures, and swallowing
                    // the second as a duplicate would lose it.
                    PaymentEvent::Failed {
                        payment_id,
                        booking_id,
                        reason: reason.clone(),
                    },
                    booking_id,
                    format!("payment-failed:{payment_id}:{reason}"),
                    payment_id,
                )
            }

            // Not ours, or a type we don't handle. The caller still answers 200.
            Outcome::Ignored => return Ok(()),
        };

        let mut conn = db::conn(&self.db).await?;

        conn.transaction::<_, MyError, _>(|conn| {
            async move {
                let version = shared::next_version!(conn, shared::schema::payment::payment, &payment_id)?;

                // `status = ANY(UNPAID)` is the guard that makes a redelivered webhook a
                // no-op, and it runs in the same transaction as the event rather than a
                // projector's moment later.
                match &event {
                    PaymentEvent::Succeeded { intent_id, .. } => {
                        PaymentRepository::transition(
                            conn,
                            payment_id,
                            &status::UNPAID,
                            PaymentPatch::succeeded(intent_id.clone()),
                        )
                        .await?;
                    }
                    PaymentEvent::Failed { reason, .. } => {
                        PaymentRepository::transition(
                            conn,
                            payment_id,
                            &status::UNPAID,
                            PaymentPatch::failed(reason.clone()),
                        )
                        .await?;
                    }
                    // `handle_webhook` builds only the two above.
                    _ => {}
                }
                shared::set_version!(conn, "payment", shared::schema::payment::payment, &payment_id, version)?;

                // Same as above: the id is what makes a redelivered webhook a no-op.
                let mut envelope =
                    Envelope::new(event, None, aggregate_id("payment", &payment_id), version);
                envelope.event_id = Uuid::new_v5(&Uuid::NAMESPACE_OID, dedupe.as_bytes());

                outbox::enqueue(conn, &payment_subject(&booking_id), &envelope).await?;
                Ok(())
            }
            .scope_boxed()
        })
        .await?;

        Ok(())
    }

    /// A host's settled income and what they have already taken out.
    ///
    /// Two queries over two tables rather than one join: each repository answers for
    /// its own table, and the subtraction is arithmetic that belongs here.
    ///
    /// Takes a connection rather than reaching for `self.db`, which is what lets
    /// `request_payout` run it **inside** its transaction, after the lock.
    ///
    /// `&mut AsyncPgConnection` and not a pool: this issues two statements, and an
    /// executor is consumed per statement — so the two halves have to share one
    /// borrow, reborrowed for each.
    pub async fn earnings_with(
        &self,
        conn: &mut diesel_async::AsyncPgConnection,
        host_id: &Uuid,
    ) -> MyResult<Earnings> {
        let cutoff = Utc::now() - chrono::Duration::seconds(self.settlement_secs);
        Ok(Earnings {
            earned_cents: PaymentRepository::earned(&mut *conn, host_id, cutoff).await?,
            paid_out_cents: PayoutRepository::total_for(&mut *conn, host_id).await?,
        })
    }

    // There was a read-only sibling here, `earnings`, behind `GET /api/payment/earnings`.
    // Both are gone: reading is view-service's job, and `GET /api/view/host/balance`
    // answers the same question over the projection.
    //
    // `earnings_with` stays because it is not the same thing. It has exactly one caller,
    // `request_payout` below, and it runs *inside* that transaction under the advisory
    // lock — which is the only reason two double-clicked withdrawals cannot both be
    // paid. A balance computed anywhere else can lag; this one cannot, and that is the
    // whole distinction.

    /// "Take the money out." Nothing leaves any real account.
    ///
    /// The amount is computed here and a client-supplied figure is never read.
    ///
    /// # The two halves that stop a double withdrawal
    ///
    /// A balance is derived, never stored, so two double-clicked requests read the
    /// same available amount and insert two *different* payout rows — different keys,
    /// nothing collides, money out twice. Both of the following are needed, and
    /// neither works alone:
    ///
    /// 1. **The advisory lock**, taken first. There is no row to lock: the row that
    ///    changes the answer is the payout that does not exist yet. A `host` table used
    ///    to exist purely to hold a version to bump, which under TiKV manufactured a
    ///    write conflict — under Read Committed it would not conflict at all, so the
    ///    table is gone and the lock is taken on the host id instead. See
    ///    `PayoutRepository::lock_host`.
    /// 2. **The balance read, moved inside the transaction and below the lock.** It
    ///    used to run before the transaction opened, which made the loser's figure stale no matter
    ///    what was locked afterwards. Read Committed gives this statement a fresh
    ///    snapshot containing the winner's payout, so the loser computes 0 and falls
    ///    into the 422 below.
    ///
    /// The loser therefore gets "You have no settled earnings yet" rather than a
    /// conflict to retry — which is the honest answer, because by then there genuinely
    /// is nothing left to withdraw.
    pub async fn request_payout(
        &self,
        host_id: &Uuid,
        requested_cents: i64,
    ) -> MyResult<(String, i64)> {
        let mut conn = db::conn(&self.db).await?;

        let (await_token, amount_cents) = conn
            .transaction::<_, MyError, _>(|conn| {
                async move {
                    // First statement in the transaction. Everything below depends on it.
                    PayoutRepository::lock_host(conn, host_id).await?;

                    // AFTER the lock, never before.
                    let available_cents =
                        self.earnings_with(conn, host_id).await?.available_cents();

                    // The client's figure meets the server's here, and only here. `check` never
                    // clamps — a request for more than there is fails and names what there is,
                    // rather than quietly paying out a different number than the screen showed.
                    let amount_cents = match policy::payout::check(requested_cents, available_cents)
                    {
                        Ok(amount) => amount,
                        Err(rejection) => return Err(refused(rejection)),
                    };

                    let payout_id = Uuid::now_v7();
                    let requested = PaymentEvent::PayoutRequested {
                        payout_id,
                        host_id: *host_id,
                        amount_cents,
                        requested_at: Utc::now(),
                    };

                    let payout_version = shared::next_version!(conn, shared::schema::payment::payout, &payout_id)?;
                    PayoutRepository::upsert(
                        conn,
                        Payout::requested(&requested, payout_version).ok_or_else(|| {
                            MyError::Bus("payout event is not a PayoutRequested".into())
                        })?,
                    )
                    .await?;

                    let envelope = Envelope::new(
                        requested,
                        Some(*host_id),
                        aggregate_id("payout", &payout_id),
                        payout_version,
                    );
                    // The payout's own version is what the client waits on — view-service
                    // records that, not the host counter this transaction contended over.
                    let await_token = format_version(&envelope.aggregate, envelope.version);

                    outbox::enqueue(conn, &payout_subject(host_id), &envelope).await?;
                    // Both values are computed INSIDE the lock, so both leave the
                    // transaction together — `amount_cents` is what the balance said at
                    // the moment it was held, and reporting a figure read outside it
                    // would be the very race the lock exists to close.
                    Ok((await_token, amount_cents))
                }
                .scope_boxed()
            })
            .await?;

        Ok((await_token, amount_cents))
    }

    /// Re-emits everything on this stream as the events that reproduce it, for a
    /// consumer that needs rebuilding. See [`outbox::backfill`] for what this is and is
    /// not.
    ///
    /// **Payments as well as payouts now.** This used to be payouts alone, on the
    /// argument that a payout was all PAYMENTS had downstream and that re-emitting
    /// `Succeeded` would only wake this service's own settlement worker for nothing.
    /// Half of that is obsolete — view-service projects the payments too, and a read
    /// model that cannot be rebuilt is not a read model — and the other half is now
    /// handled where it belongs: `bus::worker` acks a backfilled envelope without
    /// running any handler, so no worker in any service reacts to one.
    ///
    /// A payment takes two events: the `Created` that brings the row into existence and
    /// the one transition that gives it its status. Both carry the same aggregate
    /// version, which is the row's — a rebuild lands on the version the live path would
    /// have left.
    ///
    /// A payout takes one; it is written once and never changes.
    pub async fn backfill(&self) -> MyResult<usize> {
        let mut sent = 0;

        let mut read = db::conn(&self.db).await?;
        for payment in PaymentRepository::all(&mut read).await? {
            let created_at = payment.created_at;
            let created = PaymentEvent::Created(PaymentCreated {
                payment_id: payment.id,
                booking_id: payment.booking_id,
                host_id: payment.host_id,
                renter_id: payment.renter_id,
                session_id: payment.session_id.clone(),
                amount_cents: payment.amount_cents,
                created_at,
            });

            let mut events = vec![(created_at, created)];
            if let Some(terminal) = Self::terminal_event(&payment) {
                events.push(terminal);
            }

            sent += outbox::backfill(
                &self.db,
                // The booking-keyed subject the original went out on. A backfill has no
                // business landing on a different one: that subject is what orders one
                // payment's events, and the projector's lanes are partitioned by it.
                &payment_subject(&payment.booking_id),
                &aggregate_id("payment", &payment.id),
                payment.version,
                events,
            )
            .await?;
        }

        let mut read = db::conn(&self.db).await?;
        for payout in PayoutRepository::all(&mut read).await? {
            let requested_at = payout.created_at.into();

            sent += outbox::backfill(
                &self.db,
                // Keyed by host, like the original — that subject is what serialises
                // one host's withdrawals, and a backfill has no business landing on
                // a different one.
                &payout_subject(&payout.host_id),
                &aggregate_id("payout", &payout.id),
                payout.version,
                [(
                    requested_at,
                    PaymentEvent::PayoutRequested {
                        payout_id: payout.id,
                        host_id: payout.host_id,
                        amount_cents: payout.amount_cents,
                        requested_at,
                    },
                )],
            )
            .await?;
        }

        tracing::info!(events = sent, "payments and payouts backfilled");
        Ok(sent)
    }

    /// The one event that moves a payment from `created` to where it ended up, rebuilt
    /// from the row, or `None` for a payment that is still `created`.
    ///
    /// Two rows deliberately produce no terminal event:
    ///
    /// - A `succeeded` row with no `intent_id`, or a `failed` one with no reason. Both
    ///   are written in the same statement as their status, so neither combination
    ///   should exist; skipping is the honest answer for a row that does, rather than
    ///   inventing a Stripe handle.
    /// - A `refunded` row from before `refunded_at` existed. It is re-emitted as
    ///   `Succeeded` instead — which it certainly was, before it was refunded. That
    ///   matches what the live read does with such a row: `WalletRepository::find_month`
    ///   filters on `refunded_at IS NOT NULL`, so the refund is absent from the history
    ///   either way, and the balance stays right because the booking behind it is
    ///   cancelled and stops matching there.
    fn terminal_event(payment: &Payment) -> Option<(DateTime<Utc>, PaymentEvent)> {
        let payment_id = payment.id;
        let booking_id = payment.booking_id;
        let succeeded = |intent_id: String| PaymentEvent::Succeeded {
            payment_id,
            booking_id,
            intent_id,
        };

        match payment.status.as_str() {
            status::SUCCEEDED => Some((payment.created_at, succeeded(payment.intent_id.clone()?))),

            status::FAILED => Some((
                payment.created_at,
                PaymentEvent::Failed {
                    payment_id,
                    booking_id,
                    reason: payment.failure_reason.clone()?,
                },
            )),

            status::REFUNDED => match (payment.refund_id.clone(), payment.refunded_at) {
                (Some(refund_id), Some(refunded_at)) => Some((
                    refunded_at,
                    PaymentEvent::Refunded {
                        payment_id,
                        booking_id,
                        refund_id,
                        amount_cents: payment.amount_cents,
                        refunded_at,
                    },
                )),
                // Refunded before the column existed: as far as anything downstream can
                // honestly be told, this payment succeeded.
                _ => Some((payment.created_at, succeeded(payment.intent_id.clone()?))),
            },

            status::EXPIRED => Some((
                payment.created_at,
                PaymentEvent::SessionExpired {
                    payment_id,
                    booking_id,
                },
            )),

            // `created`: the `Created` event is the whole of its history.
            _ => None,
        }
    }
}

/// A refused withdrawal, as the screen should read it.
///
/// All three are **422**, not 409: nothing here is a conflict to retry. The balance was
/// read under the lock, so by the time this answers the figure it names is the true
/// one, and repeating the same request would get the same refusal.
///
/// The detail is written for a host, in euros, and reaches the form verbatim — which is
/// also why `AboveAvailable` carries the number rather than saying "too much".
fn refused(rejection: Rejection) -> MyError {
    let (title, detail) = match rejection {
        Rejection::NothingAvailable => (
            "Nothing to Withdraw",
            "You have no settled earnings yet.".to_string(),
        ),
        Rejection::BelowMinimum => (
            "Below the Minimum",
            format!(
                "The smallest withdrawal is €{}.",
                shared::domain_models::payment::payout::MIN_CENTS / 100
            ),
        ),
        Rejection::AboveAvailable { available_cents } => (
            "More Than Available",
            format!(
                "You have €{}.{:02} available to withdraw.",
                available_cents / 100,
                available_cents % 100
            ),
        ),
    };

    MyError::api(StatusCode::UNPROCESSABLE_ENTITY, title, detail)
}
