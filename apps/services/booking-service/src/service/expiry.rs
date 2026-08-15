//! Collects holds whose expiry has passed.
//!
//! This is what lets `spot.booked` mean "taken" with no qualification. The
//! alternative — carrying `hold_until` into that map and having every reader skip
//! lapsed entries — leaves a lapsed hold blocking a slot *forever* on a spot that
//! sees no further events, and puts the same filtering invariant in three separate
//! readers who each have to remember it.
//!
//! The cost of moving it here is that this task is load-bearing: with no read-time
//! filter behind it, a sweeper that stops means holds that never release. Hence
//! `run` below can only ever log and continue — nothing propagates out of the loop.

use std::sync::Arc;
use std::time::Duration;

use async_nats::jetstream::Context;
use shared::{
    error::myerror::MyResult,
    events::{
        Envelope,
        booking::{BookingEvent, ReleaseReason},
        booking_subject,
    },
};
use uuid::Uuid;

use crate::repository::booking_repository::BookingRepository;

const TICK: Duration = Duration::from_secs(60);

/// Cap per tick so one enormous backlog can't monopolise a tick or a publish batch.
/// Whatever is left is picked up on the next pass a minute later.
const BATCH: usize = 200;

pub fn spawn(js: Context, repository: Arc<BookingRepository>) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(TICK);
        loop {
            ticker.tick().await;
            // Deliberately swallowed. A failed tick is retried a minute later; a
            // propagated error would end the task, and nothing else frees a hold.
            if let Err(e) = sweep(&js, &repository).await {
                tracing::error!(error = %e, "expiry sweep failed; retrying next tick");
            }
        }
    });
}

async fn sweep(js: &Context, repository: &BookingRepository) -> MyResult<()> {
    let lapsed = repository.lapsed_holds(BATCH).await?;
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
        // `actor_id: None` — nobody requested this, the clock did.
        let mut envelope = Envelope::new(event, None);
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
        if let Err(e) =
            bus::publish(js, booking_subject(&hold.spot_shard, &spot_id), &envelope).await
        {
            tracing::error!(booking = %booking_id, error = %e, "failed to publish expiry");
        }
    }
    Ok(())
}
