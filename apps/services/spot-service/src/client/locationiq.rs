//! The only place LocationIQ is named.
//!
//! Two calls against the same provider, answering two different questions: what a
//! half-typed address might be ([`autocomplete`], which a client sees), and where an
//! address actually is ([`geocode`], which only `SpotService::create_spot` sees and which
//! is the reason a submitted coordinate is never believed).
//!
//! **No typed mirror of the provider's body.** Both endpoints answer the same
//! Nominatim-shaped JSON, and every field either call reads is optional there — so a
//! struct per endpoint would restate what `serde_json::Value` already does (a missing key
//! is `Null`), with the field names still spelled as strings in serde attributes rather
//! than in the pluck. What replaces the types is the tests at the bottom, over recorded
//! bodies: they catch a misspelled key, which a struct never did.

use std::sync::OnceLock;

use serde_json::Value;
use shared::error::myerror::{ContextExt, MyResult};
use shared::responses::spot::AutocompleteAddressResponse;

use crate::CONFIG;

fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(reqwest::Client::new)
}

/// Type-ahead suggestions for a partial address. LocationIQ answers 404 with an
/// `{"error": ...}` body when there's nothing to suggest (short/garbage query), so
/// a non-success status maps to an empty list rather than an error.
///
/// `addressdetails=1` is what makes one call enough: the structured breakdown comes back
/// with the suggestion, so picking one fills every field of the form with no follow-up.
pub async fn autocomplete(query: &str) -> MyResult<Vec<AutocompleteAddressResponse>> {
    let key = CONFIG.locationiq_api_key.as_str();
    let resp = client()
        .get("https://api.locationiq.com/v1/autocomplete")
        .query(&[
            ("key", key),
            ("q", query),
            ("limit", "5"),
            ("addressdetails", "1"),
            ("normalizeaddress", "1"),
            ("format", "json"),
        ])
        .send()
        .await
        .context_internal("locationiq autocomplete request failed")?;

    if !resp.status().is_success() {
        return Ok(vec![]);
    }

    let hits: Value = resp
        .json()
        .await
        .context_internal("locationiq autocomplete parse failed")?;

    Ok(hits
        .as_array()
        .map(|hits| hits.iter().map(address_from).collect())
        .unwrap_or_default())
}

/// Authoritative forward-geocode used to validate a submitted address. Returns
/// `Some((lng, lat))` on a confident hit, `None` when the address can't be located.
///
/// Answers no request — the pair goes into the event, which is why a client's own
/// coordinates are never read.
pub async fn geocode(address: &str) -> MyResult<Option<(f64, f64)>> {
    let key = CONFIG.locationiq_api_key.as_str();
    let resp = client()
        .get("https://us1.locationiq.com/v1/search")
        .query(&[
            ("key", key),
            ("q", address),
            ("format", "json"),
            ("limit", "1"),
        ])
        .send()
        .await
        .context_internal("locationiq search request failed")?;

    // 404 + {"error": "Unable to geocode"} => no confident match.
    if !resp.status().is_success() {
        return Ok(None);
    }

    let hits: Value = resp
        .json()
        .await
        .context_internal("locationiq search parse failed")?;

    Ok(first_point(&hits))
}

/// One hit's address components, in our shape.
///
/// The text fields collapse "absent" and "empty" into the same thing on purpose: an empty
/// `city` and no `city` at all are the same answer for a form, and [`text`] is what makes
/// them one value rather than two cases.
fn address_from(hit: &Value) -> AutocompleteAddressResponse {
    let a = &hit["address"];
    let pick = |key: &str| text(&a[key]).unwrap_or_default();

    let line1 = join(" ", [pick("road"), pick("house_number")]);
    let city = pick("city");
    let postal_code = pick("postcode");
    let region = text(&a["state"]);
    let country = pick("country");

    // Built from the components rather than from `display_name`, which includes `county`
    // — dropped here since it's not always meaningful. Same shape the frontend rebuilds
    // in CreateSpotForm, and the string the create form submits for re-geocoding.
    let formatted = join(
        ", ",
        [
            line1.clone(),
            join(" ", [postal_code.clone(), city.clone()]),
            region.clone().unwrap_or_default(),
            country.clone(),
        ],
    );

    AutocompleteAddressResponse {
        line1,
        line2: None, // OSM has no clean line2; left for manual entry.
        city,
        postal_code,
        region,
        country,
        formatted,
        lat: coord(&hit["lat"]),
        lng: coord(&hit["lon"]),
    }
}

/// The first hit's point, as `(lng, lat)`.
///
/// Indexing a `Value` past the end of an array — or into one that isn't an array at all —
/// is `Null`, so "no results" needs no branch of its own.
///
// ponytail: confidence = "got a result"; tighten with place_rank/importance
// thresholds if vague matches slip through.
fn first_point(hits: &Value) -> Option<(f64, f64)> {
    let hit = &hits[0];
    coord(&hit["lon"]).zip(coord(&hit["lat"]))
}

/// One coordinate. LocationIQ sends them as strings (`"50.85"`); `as_f64` costs one call
/// and covers the number a saner API would have sent.
///
/// `None` rather than `0.0` for anything unparseable: a suggestion with no point is still
/// an address, and `0, 0` is a place the map would fly to.
fn coord(v: &Value) -> Option<f64> {
    v.as_f64().or_else(|| v.as_str()?.parse().ok())
}

/// A present, non-empty string, or `None`.
///
/// The emptiness check is the half `as_str` doesn't do: the provider sends `""` for
/// components it has no value for, and `Some("")` in a response is a field a form would
/// render as filled in.
fn text(v: &Value) -> Option<String> {
    v.as_str().filter(|s| !s.is_empty()).map(str::to_owned)
}

/// The parts that have something in them, joined. Keeps an absent `house_number` from
/// leaving a trailing space, and an absent `region` from leaving `", ,"`.
fn join<const N: usize>(sep: &str, parts: [String; N]) -> String {
    parts
        .iter()
        .filter(|s| !s.is_empty())
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(sep)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A recorded `/v1/autocomplete` hit, which is what the field names are checked
    /// against now that nothing declares them in a struct.
    const HIT: &str = r#"{
        "place_id": "322", "display_name": "12, Rue Neuve, 1000 Brussels, Brabant, Belgium",
        "lat": "50.8503", "lon": "4.3517",
        "address": {
            "house_number": "12", "road": "Rue Neuve", "suburb": "Centre",
            "city": "Brussels", "county": "Brussels-Capital", "state": "Brabant",
            "postcode": "1000", "country": "Belgium", "country_code": "be"
        }
    }"#;

    #[test]
    fn maps_a_recorded_autocomplete_hit_onto_every_field() {
        let address = address_from(&serde_json::from_str(HIT).unwrap());

        assert_eq!(address.line1, "Rue Neuve 12");
        assert_eq!(address.line2, None);
        assert_eq!(address.city, "Brussels");
        assert_eq!(address.postal_code, "1000");
        assert_eq!(address.region.as_deref(), Some("Brabant"));
        assert_eq!(address.country, "Belgium");
        // `county` is not in it, and `display_name`'s ordering is not what we send.
        assert_eq!(
            address.formatted,
            "Rue Neuve 12, 1000 Brussels, Brabant, Belgium"
        );
        assert_eq!((address.lat, address.lng), (Some(50.8503), Some(4.3517)));
    }

    /// Every component is optional at the provider, so a hit carrying none of them has to
    /// come back as a set of empty fields rather than a parse error.
    #[test]
    fn a_hit_with_no_address_details_is_empty_rather_than_an_error() {
        let address = address_from(&serde_json::from_str(r#"{"lat":"1","lon":"2"}"#).unwrap());

        assert_eq!(address.line1, "");
        assert_eq!(address.city, "");
        assert_eq!(address.postal_code, "");
        assert_eq!(address.region, None);
        assert_eq!(address.country, "");
        assert_eq!(address.formatted, "");
    }

    /// The separators are only correct if the missing parts are dropped rather than
    /// joined as empty strings.
    #[test]
    fn absent_components_leave_no_stray_separators() {
        let hit = serde_json::from_str(
            r#"{"address":{"road":"Rue Neuve","city":"Brussels","country":"Belgium"}}"#,
        )
        .unwrap();

        let address = address_from(&hit);
        assert_eq!(address.line1, "Rue Neuve");
        assert_eq!(address.formatted, "Rue Neuve, Brussels, Belgium");
    }

    #[test]
    fn parses_search_hit_to_lng_lat() {
        let hits = serde_json::from_str(r#"[{"lat":"50.85","lon":"4.35"}]"#).unwrap();
        assert_eq!(first_point(&hits), Some((4.35, 50.85)));
    }

    #[test]
    fn empty_search_is_none() {
        assert_eq!(first_point(&serde_json::from_str("[]").unwrap()), None);
    }

    /// A hit whose coordinates are missing or junk answers `None`, never `0, 0`.
    ///
    /// That used to be `parse().unwrap_or_default()`, which put a suggestion in the Gulf
    /// of Guinea and gave the map somewhere real to fly to.
    #[test]
    fn unparseable_coordinates_are_absent_rather_than_null_island() {
        assert_eq!(
            first_point(&serde_json::from_str(r#"[{"lat":"n/a"}]"#).unwrap()),
            None
        );

        let address = address_from(&serde_json::from_str(r#"{"lat":"","lon":"4.35"}"#).unwrap());
        assert_eq!((address.lat, address.lng), (None, Some(4.35)));
    }
}
