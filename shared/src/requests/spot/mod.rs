//! Request bodies for spot-service, one file per form.
//!
//! Split by what submits them: [`create`] and [`update`] are the two halves of the
//! listing form, [`fields`] holds the values they share as newtypes that own their
//! rules, and [`rules`] holds the two field rules that are not worth a type.
//!
//! There is no `AddressRequest` or `AvailabilityRequest` any more — both were the
//! stored type restated, and the rules that were the reason for the split now sit
//! on `general_models::spot` itself. Only the rules that depend on *when* a form was
//! submitted stayed behind, in [`availability`].
//!
//! Re-exported flat, so every caller still writes
//! `shared::requests::spot::CreateSpotRequest` and the split is invisible outside.

mod availability;
mod create;
mod fields;
mod rules;
mod update;

pub use create::CreateSpotRequest;
pub use fields::{Description, Title};
pub use update::UpdateSpotRequest;

/// `GET /api/spot/address/suggest?q=…` — the type-ahead's partial address.
///
/// No `Validate`, and the route takes it as a plain `Query`. A short or garbage `q`
/// is not an error: LocationIQ answers 404 for it and `locationiq::autocomplete`
/// maps that to an empty list, which is exactly what a type-ahead wants. A length
/// rule here would turn "no suggestions yet" into a 422 and change nothing the user
/// sees.
#[derive(serde::Deserialize)]
pub struct AddressSuggestQuery {
    pub q: String,
}

/// Builders shared by the tests in the child modules.
///
/// Here rather than duplicated per file: `update` is defined as "a create minus
/// the address", and the test that pins the two schemas together needs to build
/// both from one description or it proves nothing.
#[cfg(test)]
pub(super) mod fixtures {
    use std::collections::HashMap;

    use super::*;
    use crate::general_models::spot::{Address, Availability, TimeSlot, WeeklyAvailability};

    pub fn slot(start: &str, end: &str) -> TimeSlot {
        TimeSlot {
            start: start.into(),
            end: end.into(),
        }
    }

    /// Exactly what media-service mints, built through the same function so the
    /// two cannot drift. Installs the test origin as a side effect — `BASE` is
    /// process-wide and every builder here needs it set.
    pub fn image_url() -> String {
        crate::media::init_test_base();
        crate::media::url_for(
            crate::media::PREFIX_SPOTS,
            "019fd9a1a3cb7d12b96249db33e2a909.jpeg",
        )
    }

    /// The newtypes only build through `Deserialize`, which is the point — there is
    /// no constructor that skips the rule. Tests go in the same way a request does.
    fn json<T: serde::de::DeserializeOwned>(v: &str) -> T {
        serde_json::from_value(v.into()).unwrap()
    }

    pub fn create(price_cents: i64, weekly_monday: Vec<TimeSlot>) -> CreateSpotRequest {
        CreateSpotRequest {
            title: json("A valid spot title"),
            description: Some(json("A description that is comfortably long enough.")),
            price_per_hour_cents: price_cents,
            address: Address {
                line1: "1 Main St".into(),
                line2: None,
                city: "Brussels".into(),
                postal_code: "1000".into(),
                region: None,
                country: "Belgium".into(),
                formatted: "1 Main St, 1000 Brussels, Belgium".into(),
            },
            availability: Availability {
                weekly: WeeklyAvailability {
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
            images: vec![image_url()],
        }
    }

    /// The same listing, as an edit. Built *from* `create` on purpose — see
    /// `update_applies_the_same_rules_as_create`.
    pub fn update(price_cents: i64, weekly_monday: Vec<TimeSlot>) -> UpdateSpotRequest {
        let create = create(price_cents, weekly_monday);
        UpdateSpotRequest {
            title: Some(create.title),
            description: create.description,
            price_per_hour_cents: Some(create.price_per_hour_cents),
            availability: Some(create.availability),
            images: Some(create.images),
            active: None,
        }
    }
}
