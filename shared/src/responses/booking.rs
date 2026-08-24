use axum::{Json, http::StatusCode};
use serde::Serialize;
use uuid::Uuid;

/// The one write in the system that answers with more than a seq.
///
/// Here rather than in [`super::common`] because nothing else needs the shape:
/// spot-service used to return an id too and no longer does, since its create form
/// navigates to a list and refetches. Move it up a module if a second caller ever
/// earns one.
#[derive(Serialize)]
pub struct CreatedResponse {
    /// The uuid, hyphenated. The client opens a Stripe Checkout Session from it
    /// straight away, which is why it has to come back here — the booking is minted
    /// server-side and no projection has caught up yet.
    pub id: Uuid,
    /// `"BOOKINGS:812"` — where this write landed in the log. The client echoes it
    /// back on its next read so a load balancer can't route it to an instance that
    /// hasn't projected this event yet.
    pub seq: String,
}

impl CreatedResponse {
    /// 202 for the same reason as [`super::common::accepted`].
    ///
    /// Takes `self` rather than the parts, because `BookingService::create_booking`
    /// builds the value — it is the thing that knows which stream the event went
    /// to. The route only decides the status code.
    pub fn accepted(self) -> (StatusCode, Json<Self>) {
        (StatusCode::ACCEPTED, Json(self))
    }
}
