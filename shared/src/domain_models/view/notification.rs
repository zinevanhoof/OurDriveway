use chrono::{DateTime, Utc};
use diesel::deserialize::FromSqlRow;
use diesel::expression::AsExpression;
use diesel::prelude::*;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// What a notification says, one variant per kind. Stored whole in `notification.data`
/// and sent to the client as-is, so adding a kind is a variant here, a projector arm
/// that inserts it, and a branch in the drawer — no schema change and no new route.
///
/// Internally tagged, so the client reads `{ kind: "rate_booking", bookingId, … }` as
/// a discriminated union.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, AsExpression, FromSqlRow)]
#[diesel(sql_type = diesel::sql_types::Jsonb)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NotificationPayload {
    /// A confirmed booking is over and the renter has not rated it. Handled by `Rated`,
    /// or by `Cancelled` before it ever shows.
    #[serde(rename_all = "camelCase")]
    RateBooking {
        booking_id: Uuid,
        /// Copied at confirmation. `None` when the spot had not been projected yet —
        /// the streams have no cross-stream order.
        spot_title: Option<String>,
    },
    /// Someone paid for a booking on the host's spot. Nothing finishes it by itself, so
    /// the host dismisses it by tapping it — `UserEvent::NotificationDismissed`. A
    /// cancellation handles it too: the booking it announces is gone.
    #[serde(rename_all = "camelCase")]
    SpotBooked {
        booking_id: Uuid,
        spot_id: Uuid,
        spot_title: Option<String>,
        /// The renter's first name, `None` when their user row had not been projected.
        renter_name: Option<String>,
    },
}

crate::jsonb_column!(NotificationPayload);

impl NotificationPayload {
    /// The `kind` column, which is the serde tag. Kept in step by the test below.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::RateBooking { .. } => kinds::RATE_BOOKING,
            Self::SpotBooked { .. } => kinds::SPOT_BOOKED,
        }
    }

    /// The `subject_id` column: what an event that handles this one will name.
    pub fn subject_id(&self) -> Uuid {
        match self {
            Self::RateBooking { booking_id, .. } | Self::SpotBooked { booking_id, .. } => {
                *booking_id
            }
        }
    }
}

pub mod kinds {
    pub const RATE_BOOKING: &str = "rate_booking";
    pub const SPOT_BOOKED: &str = "spot_booked";
    /// Every kind, for checking a `kind` that arrives from a client.
    pub const ALL: [&str; 2] = [RATE_BOOKING, SPOT_BOOKED];
}

/// The `notification` table in the read model.
#[derive(Clone, Debug, Queryable, Selectable, Insertable)]
#[diesel(table_name = crate::schema::view::notification)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct ViewNotification {
    pub subject_id: Uuid,
    pub kind: String,
    pub user_id: Uuid,
    pub data: NotificationPayload,
    /// Not listed before this. A rating prompt is inserted at confirmation and dated at
    /// the booking's end.
    pub visible_from: DateTime<Utc>,
    /// Set once, by the event that makes it moot. Never listed again after.
    pub handled_at: Option<DateTime<Utc>>,
}

impl ViewNotification {
    pub fn new(user_id: Uuid, data: NotificationPayload, visible_from: DateTime<Utc>) -> Self {
        Self {
            subject_id: data.subject_id(),
            kind: data.kind().to_string(),
            user_id,
            data,
            visible_from,
            handled_at: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `kind` column and the JSON tag are the same string, or `handle` would miss.
    #[test]
    fn kind_matches_the_serde_tag() {
        let all = [
            NotificationPayload::RateBooking {
                booking_id: Uuid::nil(),
                spot_title: None,
            },
            NotificationPayload::SpotBooked {
                booking_id: Uuid::nil(),
                spot_id: Uuid::nil(),
                spot_title: None,
                renter_name: None,
            },
        ];
        for n in all {
            assert_eq!(serde_json::to_value(&n).unwrap()["kind"], n.kind());
            assert!(kinds::ALL.contains(&n.kind()));
        }
    }
}
