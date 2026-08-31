use garde::Validate;
use serde::Deserialize;

use super::availability::{has_any_slot, no_past_dates};
use super::fields::{Description, Title};
use super::rules::{are_spot_images, is_positive};
use crate::general_models::spot::{Address, Availability};

#[derive(Deserialize, Validate)]
#[serde(rename_all = "camelCase")]
pub struct CreateSpotRequest {
    #[garde(dive)]
    pub title: Title,
    #[garde(dive)]
    pub description: Option<Description>,
    /// EUR cents, integer. The client sends cents so no float ever reaches the
    /// money path — the form still collects euros and converts on submit.
    #[garde(custom(is_positive))]
    pub price_per_hour_cents: i64,

    /// The stored type directly. There was an `AddressRequest` here that was the
    /// same seven fields, seven `#[garde(skip)]` and a `From` impl — the address is
    /// verified by LocationIQ server-side, so there was never a rule to carry.
    #[garde(skip)]
    pub address: Address,
    /// Also the stored type: the grid's own rules live on it now. Only the two
    /// submission-time rules are applied here — see `super::availability`.
    #[garde(dive, custom(has_any_slot), custom(no_past_dates))]
    pub availability: Availability,
    /// Media URLs, in display order. The browser uploads each photo straight to R2
    /// first and sends back the whole URL media-service minted for it — no image
    /// bytes reach this service at all.
    ///
    /// The URL, not the bucket key: `are_spot_images` defers to
    /// `media::is_media_url`, which checks the origin and the `{prefix}/{32 hex}.{ext}`
    /// shape together, and a bare key satisfies neither half.
    #[garde(custom(are_spot_images))]
    pub images: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::{create, slot};
    use garde::Validate;

    #[test]
    fn valid_request_passes() {
        assert!(create(500, vec![slot("08:00", "10:00")]).validate().is_ok());
    }

    #[test]
    fn rejects_non_positive_price() {
        // Needs a slot, otherwise `has_any_slot` would fail it regardless of price.
        assert!(create(0, vec![slot("08:00", "10:00")]).validate().is_err());
    }

    #[test]
    fn rejects_a_short_title() {
        let mut r = create(500, vec![slot("08:00", "10:00")]);
        r.title = serde_json::from_value("no".into()).unwrap();
        assert!(r.validate().is_err());
    }
}
