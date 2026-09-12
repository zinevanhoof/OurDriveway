use serde::Serialize;
use uuid::Uuid;

/// What `POST /api/booking` answers a [`crate::requests::booking::CreateBookingRequest`]
/// with — the one write in the system that answers with more than a version.
///
/// Named for its request, not for its shape: spot-service used to return an id too and
/// no longer does, since its create form navigates to a list and refetches. A second
/// write that needs an id gets its own response rather than sharing this one.
///
/// The version this write reached leaves as the `X-Version` header, not as a field
/// here — see [`super::common::X_VERSION`].
#[derive(Serialize)]
pub struct CreateBookingResponse {
    /// The uuid, hyphenated. The client opens a Stripe Checkout Session from it
    /// straight away, which is why it has to come back here — the booking is minted
    /// server-side and no projection has caught up yet.
    pub id: Uuid,
}
