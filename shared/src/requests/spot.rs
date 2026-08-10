use std::collections::HashMap;

use chrono::{NaiveDate, Utc};
use garde::Validate;
use serde::Deserialize;

use crate::general_models::spot::{Address, Availability, TimeSlot, WeeklyAvailability};

#[derive(Deserialize, Validate)]
#[serde(rename_all = "camelCase")]
pub struct CreateSpotRequest {
    #[garde(length(min = 5, max = 32))]
    pub title: String,
    #[garde(inner(length(min = 20, max = 100)))]
    pub description: Option<String>,
    /// EUR cents, integer. The client sends cents so no float ever reaches the
    /// money path — the form still collects euros and converts on submit.
    #[garde(custom(is_positive))]
    pub price_per_hour_cents: i64,

    #[garde(dive)]
    pub address: AddressRequest,
    #[garde(dive, custom(has_any_slot))]
    pub availability: AvailabilityRequest,
    /// Media keys, in display order. The browser uploads each photo straight to
    /// R2 first and sends back the keys media-service minted for them — no image
    /// bytes reach this service at all.
    #[garde(custom(are_spot_images))]
    pub images: Vec<String>,
}

/// An edit of an existing listing. Every field the host can still change, all of
/// them required — the form submits its whole state, so a partial payload would mean
/// the client silently deciding what "unchanged" is.
///
/// No address: a spot's location is fixed at creation, because the coordinates and
/// the IANA zone derived from them are what every stored `booked` string is relative
/// to. Moving a spot would reinterpret bookings already made.
#[derive(Deserialize, Validate)]
#[serde(rename_all = "camelCase")]
pub struct UpdateSpotRequest {
    #[garde(length(min = 5, max = 32))]
    pub title: String,
    #[garde(inner(length(min = 20, max = 100)))]
    pub description: Option<String>,
    #[garde(custom(is_positive))]
    pub price_per_hour_cents: i64,
    #[garde(dive, custom(has_any_slot))]
    pub availability: AvailabilityRequest,
    /// Media keys, in display order — the ones the host kept and the ones they
    /// just uploaded, already merged by the client. Same rules as create.
    #[garde(custom(are_spot_images))]
    pub images: Vec<String>,
}

/// A listing needs at least one photo, and every photo has to be one of ours.
///
/// Both halves are here rather than in the route because there is now a single
/// image list. It used to be split — kept URLs inside the JSON, new files as
/// multipart parts — so "at least one" could only be judged after merging them,
/// and lived in the handler as a hand-rolled 422.
///
/// The membership half is a trust boundary: these strings come straight back from
/// the client and land in an event that every renter renders as an `<img src>`.
/// See [`crate::media::is_media_key`].
fn are_spot_images(images: &Vec<String>, _: &()) -> garde::Result {
    require(!images.is_empty(), "Add at least one photo.")?;
    require(
        images
            .iter()
            .all(|image| crate::media::is_media_key(image, crate::media::PREFIX_SPOTS)),
        "Unknown photo.",
    )
}

/// Request variant of `Address`: deserializes, no `SurrealValue`. The address is
/// verified by LocationIQ server-side, so the fields carry no length rules.
#[derive(Deserialize, Validate)]
#[serde(rename_all = "camelCase")]
pub struct AddressRequest {
    #[garde(skip)]
    pub line1: String,
    #[garde(skip)]
    pub line2: Option<String>,
    #[garde(skip)]
    pub city: String,
    #[garde(skip)]
    pub postal_code: String,
    #[garde(skip)]
    pub region: Option<String>,
    #[garde(skip)]
    pub country: String,
    #[garde(skip)]
    pub formatted: String,
}

#[derive(Deserialize, Validate)]
pub struct AvailabilityRequest {
    #[garde(dive)]
    pub weekly: WeeklyAvailabilityRequest,
    #[garde(custom(validate_single))]
    pub single: HashMap<String, Vec<TimeSlotRequest>>,
}

#[derive(Deserialize, Validate)]
pub struct WeeklyAvailabilityRequest {
    #[garde(custom(validate_slots))]
    pub monday: Vec<TimeSlotRequest>,
    #[garde(custom(validate_slots))]
    pub tuesday: Vec<TimeSlotRequest>,
    #[garde(custom(validate_slots))]
    pub wednesday: Vec<TimeSlotRequest>,
    #[garde(custom(validate_slots))]
    pub thursday: Vec<TimeSlotRequest>,
    #[garde(custom(validate_slots))]
    pub friday: Vec<TimeSlotRequest>,
    #[garde(custom(validate_slots))]
    pub saturday: Vec<TimeSlotRequest>,
    #[garde(custom(validate_slots))]
    pub sunday: Vec<TimeSlotRequest>,
}

/// Slots are validated collectively (see `validate_slots`), so the fields
/// themselves carry no per-field rules — hence just `Deserialize`, no `SurrealValue`.
#[derive(Deserialize)]
pub struct TimeSlotRequest {
    pub start: String,
    pub end: String,
}

fn is_positive(value: &i64, _: &()) -> garde::Result {
    require(*value > 0, "Price per hour must be greater than 0")
}

/// A spot with no slots at all can never be booked, so at least one weekly or
/// one-off slot is required. Mirrors the same guard in AddSpotView.
fn has_any_slot(value: &AvailabilityRequest, _: &()) -> garde::Result {
    let w = &value.weekly;
    let weekly = [
        &w.monday,
        &w.tuesday,
        &w.wednesday,
        &w.thursday,
        &w.friday,
        &w.saturday,
        &w.sunday,
    ]
    .iter()
    .any(|slots| !slots.is_empty());

    require(
        weekly || value.single.values().any(|slots| !slots.is_empty()),
        "Add at least one availability slot.",
    )
}

/// Full mirror of the frontend weekday-slot rules: `HH:MM`, 30-minute
/// increments, end after start, and no overlaps or duplicates within the day.
///
/// `pub(crate)` because a booking request needs the identical rules — see
/// `requests/booking.rs`. Two copies of these would drift.
pub(crate) fn validate_slots(slots: &Vec<TimeSlotRequest>, _: &()) -> garde::Result {
    for slot in slots {
        check_time(&slot.start)?;
        check_time(&slot.end)?;
        // String compare is correct for zero-padded HH:MM.
        require(slot.start < slot.end, "End time must be after the start time.")?;
    }
    for (i, a) in slots.iter().enumerate() {
        for b in &slots[i + 1..] {
            require(
                !(a.start == b.start && a.end == b.end),
                "This time slot already exists.",
            )?;
            require(
                !(a.start < b.end && a.end > b.start),
                "This time slot overlaps with an existing slot.",
            )?;
        }
    }
    Ok(())
}

/// Single-day availability: date key must be today or later, and each day's
/// slots follow the same rules as the weekly ones.
pub(crate) fn validate_single(map: &HashMap<String, Vec<TimeSlotRequest>>, _: &()) -> garde::Result {
    // ponytail: past-date compared against UTC today, not the spot's timezone
    // (unknown until after geocoding). Fine ±1 day at the boundary; make it
    // tz-aware if the zone is resolved earlier.
    let today = Utc::now().date_naive();
    for (key, slots) in map {
        let date = NaiveDate::parse_from_str(key, "%Y-%m-%d")
            .map_err(|_| garde::Error::new("Invalid date"))?;
        require(date >= today, "Date cannot be in the past")?;
        validate_slots(slots, &())?;
    }
    Ok(())
}

/// `HH:MM`, 00–23 hours, 00–59 minutes, minutes a multiple of 30.
fn check_time(value: &str) -> garde::Result {
    let (h, m) = value
        .split_once(':')
        .ok_or_else(|| garde::Error::new("Invalid time"))?;
    let hour: u32 = h.parse().map_err(|_| garde::Error::new("Invalid time"))?;
    let min: u32 = m.parse().map_err(|_| garde::Error::new("Invalid time"))?;
    require(
        h.len() == 2 && m.len() == 2 && hour < 24 && min < 60 && min % 30 == 0,
        "Time must be in 30 minute increments.",
    )
}

fn require(ok: bool, msg: &'static str) -> garde::Result {
    if ok {
        Ok(())
    } else {
        Err(garde::Error::new(msg))
    }
}

impl From<AddressRequest> for Address {
    fn from(a: AddressRequest) -> Self {
        Self {
            line1: a.line1,
            line2: a.line2,
            city: a.city,
            postal_code: a.postal_code,
            region: a.region,
            country: a.country,
            formatted: a.formatted,
        }
    }
}

impl From<TimeSlotRequest> for TimeSlot {
    fn from(s: TimeSlotRequest) -> Self {
        Self {
            start: s.start,
            end: s.end,
        }
    }
}

fn slots(v: Vec<TimeSlotRequest>) -> Vec<TimeSlot> {
    v.into_iter().map(Into::into).collect()
}

impl From<WeeklyAvailabilityRequest> for WeeklyAvailability {
    fn from(w: WeeklyAvailabilityRequest) -> Self {
        Self {
            monday: slots(w.monday),
            tuesday: slots(w.tuesday),
            wednesday: slots(w.wednesday),
            thursday: slots(w.thursday),
            friday: slots(w.friday),
            saturday: slots(w.saturday),
            sunday: slots(w.sunday),
        }
    }
}

impl From<AvailabilityRequest> for Availability {
    fn from(a: AvailabilityRequest) -> Self {
        Self {
            weekly: a.weekly.into(),
            single: a.single.into_iter().map(|(k, v)| (k, slots(v))).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot(start: &str, end: &str) -> TimeSlotRequest {
        TimeSlotRequest {
            start: start.into(),
            end: end.into(),
        }
    }

    fn req(price_cents: i64, weekly_monday: Vec<TimeSlotRequest>) -> CreateSpotRequest {
        CreateSpotRequest {
            title: "A valid spot title".into(),
            description: Some("A description that is comfortably long enough.".into()),
            price_per_hour_cents: price_cents,
            address: AddressRequest {
                line1: "1 Main St".into(),
                line2: None,
                city: "Brussels".into(),
                postal_code: "1000".into(),
                region: None,
                country: "Belgium".into(),
                formatted: "1 Main St, 1000 Brussels, Belgium".into(),
            },
            availability: AvailabilityRequest {
                weekly: WeeklyAvailabilityRequest {
                    monday: weekly_monday,
                    tuesday: vec![],
                    wednesday: vec![],
                    thursday: vec![],
                    friday: vec![],
                    saturday: vec![],
                    sunday: vec![],
                },
                single: HashMap::new(),
            },
            images: vec![IMAGE_KEY.into()],
        }
    }

    /// Shaped exactly like what media-service mints — see `shared::media`.
    const IMAGE_KEY: &str = "spots/019fd9a1a3cb7d12b96249db33e2a909.jpeg";

    #[test]
    fn valid_request_passes() {
        assert!(req(500, vec![slot("08:00", "10:00")]).validate().is_ok());
    }

    #[test]
    fn rejects_non_positive_price() {
        // Needs a slot, otherwise `has_any_slot` would fail it regardless of price.
        assert!(req(0, vec![slot("08:00", "10:00")]).validate().is_err());
    }

    #[test]
    fn rejects_availability_without_any_slot() {
        assert!(req(500, vec![]).validate().is_err());
    }

    #[test]
    fn accepts_availability_with_only_a_single_date() {
        let mut r = req(500, vec![]);
        let tomorrow = (Utc::now() + chrono::Duration::days(1)).date_naive();
        r.availability
            .single
            .insert(tomorrow.to_string(), vec![slot("08:00", "10:00")]);
        assert!(r.validate().is_ok());
    }

    #[test]
    fn rejects_malformed_time() {
        assert!(req(500, vec![slot("08:15", "10:00")]).validate().is_err()); // not a 30-min increment
        assert!(req(500, vec![slot("25:00", "26:00")]).validate().is_err()); // out of range
    }

    #[test]
    fn rejects_overlapping_slots() {
        assert!(
            req(500, vec![slot("08:00", "10:00"), slot("09:00", "11:00")])
                .validate()
                .is_err()
        );
    }

    fn update(price_cents: i64, weekly_monday: Vec<TimeSlotRequest>) -> UpdateSpotRequest {
        let create = req(price_cents, weekly_monday);
        UpdateSpotRequest {
            title: create.title,
            description: create.description,
            price_per_hour_cents: create.price_per_hour_cents,
            availability: create.availability,
            images: create.images,
        }
    }

    /// An edit is held to the same rules as a create — the two structs are separate
    /// only because of the address, so this is what catches them drifting apart.
    #[test]
    fn update_applies_the_same_rules_as_create() {
        assert!(update(500, vec![slot("08:00", "10:00")]).validate().is_ok());
        assert!(update(0, vec![slot("08:00", "10:00")]).validate().is_err());
        assert!(update(500, vec![]).validate().is_err());
        assert!(update(500, vec![slot("08:15", "10:00")]).validate().is_err());
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
            r.images = images.into_iter().map(Into::into).collect();
            r.validate().is_ok()
        };

        assert!(with(vec![IMAGE_KEY]));
        assert!(!with(vec!["https://evil.example/track.png"]));
        assert!(!with(vec!["spots/../../etc/passwd"]));
        // The old scheme. Anything still holding one of these is stale data, not a
        // photo this app can serve.
        assert!(!with(vec!["/api/spot/uploads/019a.jpg"]));
        // One bad key poisons the list — an event is all-or-nothing.
        assert!(!with(vec![IMAGE_KEY, "https://evil.example/track.png"]));
        // Now garde's, not the route's: with one merged list there is no longer a
        // case where zero images is legitimate.
        assert!(!with(vec![]));
    }

    #[test]
    fn rejects_past_single_date() {
        let mut r = req(500, vec![]);
        r.availability
            .single
            .insert("2000-01-01".into(), vec![slot("08:00", "10:00")]);
        assert!(r.validate().is_err());
    }
}
