//! Releases holds whose expiry has passed.
//!
//! The third event loop, beside `projector.rs` and `worker.rs` — driven by the clock
//! rather than by a stream, which is the only difference. It answers no request, so
//! it is not one of the `service/` types.
//!
//! This is what lets a `reserved` row mean "taken" with no qualification. The
//! alternative — every availability read also filtering on `hold_until` — leaves a
//! lapsed hold blocking a slot *forever* on a spot that sees no further events, and
//! puts the same invariant in three separate readers who each have to remember it.
//!
//! The cost of moving it here is that this task is load-bearing: with no read-time
//! filter behind it, a sweeper that stops means holds that never release. Hence
//! `run` below can only ever log and continue — nothing propagates out of the loop.

use std::sync::Arc;
use std::time::Duration;

use shared::{
    domain_models::booking::status,
    error::myerror::MyResult,
    events::{
        Envelope,
        booking::{BookingEvent, ReleaseReason},
        aggregate_id, booking_subject,
    },
};
use uuid::Uuid;

use crate::repository::booking_repository::BookingRepository;
use surrealdb::{Surreal, engine::remote::ws::Client};

const TICK: Duration = Duration::from_secs(60);

/// Cap per tick so one enormous backlog can't monopolise a tick or a publish batch.
/// Whatever is left is picked up on the next pass a minute later.
const MAX_PER_SWEEP: usize = 200;

/// Sweeps forever. `tokio::spawn` is the caller's, like the projectors' and the
/// worker's — this loop is no more special than theirs.
pub async fn run(db: Arc<Surreal<Client>>) {
    let bookings = BookingRepository { q: db };
    let mut ticker = tokio::time::interval(TICK);
    loop {
        ticker.tick().await;
        // Deliberately swallowed. A failed tick is retried a minute later; a
        // propagated error would end the task, and nothing else frees a hold.
        if let Err(e) = sweep(&bookings).await {
            tracing::error!(error = %e, "sweep failed; retrying next tick");
        }
    }
}

async fn sweep(bookings: &BookingRepository) -> MyResult<()> {
    let lapsed = bookings.lapsed_holds(MAX_PER_SWEEP).await?;
    if lapsed.is_empty() {
        return Ok(());
    }
    tracing::info!(count = lapsed.len(), "releasing lapsed holds");

    for hold in lapsed {
        let (booking_id, spot_id) = (hold.id, hold.spot_id);

        let event = BookingEvent::Released {
            booking_id,
            reason: ReleaseReason::Expired,
        };
        let tx = match shared::db::begin(&bookings.q).await {
            Ok(tx) => tx,
            Err(e) => {
                tracing::error!(booking = %booking_id, error = %e, "could not open transaction");
                continue;
            }
        };
        let version = match shared::db::next_version(&tx, "booking", &booking_id).await {
            Ok(v) => v,
            Err(e) => {
                tracing::error!(booking = %booking_id, error = %e, "could not read version");
                continue;
            }
        };

        // Scoped to `reserved`, which is what stops this undoing a payment that
        // landed in the same instant — the sweeper and `confirm_paid` genuinely
        // race, and the guard is the whole answer to it.
        let moved = BookingRepository { q: &tx }
            .transition(
                booking_id,
                status::RELEASED,
                &[status::RESERVED],
                Some(ReleaseReason::Expired.as_str()),
                None,
            )
            .await;
        if let Err(e) = moved {
            tracing::error!(booking = %booking_id, error = %e, "could not release");
            continue;
        }
        if let Err(e) = shared::db::set_version(&tx, "booking", &booking_id, version).await {
            tracing::error!(booking = %booking_id, error = %e, "could not set version");
            continue;
        }

        // `actor_id: None` — nobody requested this, the clock did.
        let mut envelope =
            Envelope::new(event, None, aggregate_id("booking", &booking_id), version);
        // Deliberately NOT a v7 id, the only place in the system that isn't. It
        // rides the `Nats-Msg-Id` header `publish` already sets, so two instances
        // sweeping the same booking inside the stream's 120s duplicate_window
        // collapse to one event — and the 60s tick sits comfortably inside that.
        envelope.event_id = Uuid::new_v5(
            &Uuid::NAMESPACE_OID,
            format!("expire:{booking_id}").as_bytes(),
        );

        // No compare-and-swap: a release only ever frees slots, so it can't lose a
        // race in a way that matters. Applying it is guarded on the booking still
        // being 'reserved', which is what stops it undoing a payment that landed in
        // the same instant.
        if let Err(e) = bus::outbox::enqueue(&tx, &booking_subject(&spot_id), &envelope).await {
            tracing::error!(booking = %booking_id, error = %e, "could not enqueue release");
            continue;
        }
        if let Err(e) = tx.commit().await {
            // Nothing is lost: the hold is still `reserved` with a past
            // `hold_until`, so the next tick a minute from now picks it up again.
            tracing::error!(booking = %booking_id, error = %e, "could not commit release");
        }
    }
    Ok(())
}
