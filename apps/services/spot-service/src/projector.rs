use bus::Projector;
use chrono::{DateTime, Utc};
use shared::{
    domain_models::spot::{Spot, SpotPatch},
    error::myerror::MyResult,
    events::{STREAM_SPOTS, spot::SpotEvent},
};
use surrealdb::{engine::remote::ws::Client, method::Transaction};

use crate::repository::spot_repository::SpotRepository;

/// Applies SPOTS events to this instance's local projection — the only writer to
/// the `spot` table. Handlers publish; they never write.
///
/// Holds no connection, deliberately: `bus::Tx` owns the only one and hands this
/// a `&Transaction` per event, so there is no path from here to the database that
/// bypasses the transaction. Every arm is one statement, and the row each event
/// produces is a constructor on the model rather than a CONTENT block here.
pub struct SpotProjector;

impl Projector for SpotProjector {
    const STREAM: &'static str = STREAM_SPOTS;
    type Event = SpotEvent;

    async fn apply(
        &self,
        tx: &Transaction<Client>,
        event: SpotEvent,
        at: DateTime<Utc>,
        _seq: u64,
    ) -> MyResult<()> {
        let spots = SpotRepository { q: tx };

        match event {
            // UPSERT keyed by the event's own id, not CREATE: replay must be
            // idempotent, and a database-generated id would differ per replica.
            SpotEvent::Created(e) => spots.upsert(Spot::created(e, at)).await,

            SpotEvent::Updated(e) => {
                let spot_id = e.spot_id;
                spots.patch(spot_id, SpotPatch::updated(e, at)).await
            }

            SpotEvent::Deleted { spot_id } => spots.patch(spot_id, SpotPatch::deleted(at)).await,
        }
    }
}
