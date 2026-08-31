use garde::Validate;
use serde::Deserialize;

use super::availability::{has_any_slot, no_past_dates};
use super::fields::{Description, Title};
use super::rules::{are_spot_images, is_positive};
use crate::general_models::spot::Availability;

/// An edit of an existing listing. Every field the host can still change, and
/// every one of them optional: `None` is "leave alone", matching `SpotUpdated`.
///
/// The edit form still submits its whole state — but the live switch is this same
/// request carrying nothing but `active`, and that is the reason for the optionality.
/// A toggle that had to resubmit `availability` would put a tap on the manage
/// screen through booking-service's cancel-what-no-longer-fits path, off a read
/// that may be a moment stale.
///
/// No address: a spot's location is fixed at creation, because the coordinates and
/// the IANA zone derived from them are what every stored `booked` string is
/// relative to. Moving a spot would reinterpret bookings already made.
#[derive(Deserialize, Validate)]
#[serde(rename_all = "camelCase")]
pub struct UpdateSpotRequest {
    #[garde(dive)]
    pub title: Option<Title>,
    #[garde(dive)]
    pub description: Option<Description>,
    #[garde(inner(custom(is_positive)))]
    pub price_per_hour_cents: Option<i64>,
    #[garde(dive, custom(has_slots_if_present), custom(dates_not_past))]
    pub availability: Option<Availability>,
    /// Media URLs, in display order — the ones the host kept and the ones they
    /// just uploaded, already merged by the client. Same rules as create.
    #[garde(inner(custom(are_spot_images)))]
    pub images: Option<Vec<String>>,
    /// The live switch. Off stops new reservations; the bookings already taken are
    /// honoured, which is the whole difference from a delete.
    #[garde(skip)]
    pub active: Option<bool>,
}

/// `None` leaves the hours alone, so only a grid that was actually submitted has
/// to hold a slot. `dive` covers the slots inside it; this covers the grid being
/// empty, which no single field can see.
fn has_slots_if_present(value: &Option<Availability>, ctx: &()) -> garde::Result {
    value.as_ref().map_or(Ok(()), |a| has_any_slot(a, ctx))
}

/// Same reasoning, for the rule that cannot live on the stored type. `inner(dive)`
/// would reach the grid's own rules but not this one, which is applied to the
/// submission rather than to the value.
fn dates_not_past(value: &Option<Availability>, ctx: &()) -> garde::Result {
    value.as_ref().map_or(Ok(()), |a| no_past_dates(a, ctx))
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::{image_url, slot, update};
    use garde::Validate;

    /// An edit is held to the same rules as a create — the two structs are
    /// separate only because of the address, so this is what catches them drifting
    /// apart.
    #[test]
    fn update_applies_the_same_rules_as_create() {
        assert!(update(500, vec![slot("08:00", "10:00")]).validate().is_ok());
        assert!(update(0, vec![slot("08:00", "10:00")]).validate().is_err());
        assert!(update(500, vec![]).validate().is_err());
        assert!(
            update(500, vec![slot("08:15", "10:00")])
                .validate()
                .is_err()
        );
    }

    /// The live switch is this request with everything else absent. If any field
    /// ever goes back to being required, the toggle 422s instead of flipping.
    #[test]
    fn the_live_switch_is_a_valid_edit_on_its_own() {
        assert!(
            super::UpdateSpotRequest {
                title: None,
                description: None,
                price_per_hour_cents: None,
                availability: None,
                images: None,
                active: Some(false),
            }
            .validate()
            .is_ok()
        );
    }

    /// The image list is client-supplied and ends up in an event every renter
    /// renders, so only keys media-service minted are allowed through.
    ///
    /// `shared::media` covers the key grammar itself; this is about the list being
    /// wired into both request types.
    #[test]
    fn images_must_be_our_own_media_keys() {
        let with = |images: Vec<&str>| {
            let mut r = update(500, vec![slot("08:00", "10:00")]);
            r.images = Some(images.into_iter().map(Into::into).collect());
            r.validate().is_ok()
        };

        assert!(with(vec![&image_url()]));
        assert!(!with(vec!["https://evil.example/track.png"]));
        // Right shape, wrong origin — the check this scheme exists for.
        assert!(!with(vec![
            "https://evil.example/spots/019fd9a1a3cb7d12b96249db33e2a909.jpeg"
        ]));
        assert!(!with(vec!["spots/../../etc/passwd"]));
        // A bare key, i.e. the scheme this replaced.
        assert!(!with(vec!["spots/019fd9a1a3cb7d12b96249db33e2a909.jpeg"]));
        // The old scheme. Anything still holding one of these is stale data, not a
        // photo this app can serve.
        assert!(!with(vec!["/api/spot/uploads/019a.jpg"]));
        // One bad key poisons the list — an event is all-or-nothing.
        assert!(!with(vec![&image_url(), "https://evil.example/track.png"]));
        // Now garde's, not the route's: with one merged list there is no longer a
        // case where zero images is legitimate.
        assert!(!with(vec![]));
    }
}
