use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use shared::error::myerror::{ContextExt, MyResult};

use crate::CONFIG;

/// A LocationIQ suggestion mapped into our `Address` shape (camelCase to match the
/// frontend). The autocomplete response already carries the structured breakdown,
/// so the client fills every field from the picked item with no follow-up call.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedAddress {
    pub line1: String,
    pub line2: Option<String>,
    pub city: String,
    pub postal_code: String,
    pub region: Option<String>,
    pub country: String,
    pub formatted: String,
    pub lat: f64,
    pub lng: f64,
}

/// One `/v1/autocomplete` result (`addressdetails=1`). Unknown fields (name,
/// suburb, county, country_code, ...) are ignored — we only pull what maps to
/// our `Address`. lat/lon come back as strings, same as `/v1/search`.
#[derive(Deserialize)]
struct IqResult {
    #[serde(default)]
    address: IqAddress,
    #[serde(default)]
    lat: String,
    #[serde(default)]
    lon: String,
}

#[derive(Deserialize, Default)]
struct IqAddress {
    house_number: Option<String>,
    road: Option<String>,
    city: Option<String>,
    postcode: Option<String>,
    state: Option<String>,
    country: Option<String>,
}

/// One `/v1/search` result. lat/lon come back as strings.
#[derive(Deserialize)]
struct IqPoint {
    lat: String,
    lon: String,
}

impl From<IqResult> for ResolvedAddress {
    fn from(r: IqResult) -> Self {
        let a = r.address;
        let line1 = [a.road.as_deref(), a.house_number.as_deref()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" ");
        let city = a.city.unwrap_or_default();
        let postal_code = a.postcode.unwrap_or_default();
        let region = a.state;
        let country = a.country.unwrap_or_default();

        // Build `formatted` from the components rather than display_name, which
        // includes `county` — dropped here since it's not always meaningful. Same
        // shape the frontend rebuilds in CreateSpotForm.
        let postcode_city = [postal_code.as_str(), city.as_str()]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        let formatted = [
            line1.as_str(),
            postcode_city.as_str(),
            region.as_deref().unwrap_or_default(),
            country.as_str(),
        ]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(", ");

        Self {
            line1,
            line2: None, // OSM has no clean line2; left for manual entry.
            city,
            postal_code,
            region,
            country,
            formatted,
            lat: r.lat.parse().unwrap_or_default(),
            lng: r.lon.parse().unwrap_or_default(),
        }
    }
}

fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(reqwest::Client::new)
}

/// Type-ahead suggestions for a partial address. LocationIQ answers 404 with an
/// `{"error": ...}` body when there's nothing to suggest (short/garbage query), so
/// a non-success status maps to an empty list rather than an error.
pub async fn autocomplete(query: &str) -> MyResult<Vec<ResolvedAddress>> {
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

    let results: Vec<IqResult> = resp
        .json()
        .await
        .context_internal("locationiq autocomplete parse failed")?;
    Ok(results.into_iter().map(ResolvedAddress::from).collect())
}

/// Authoritative forward-geocode used to validate a submitted address. Returns
/// `Some((lng, lat))` on a confident hit, `None` when the address can't be located.
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

    let results: Vec<IqPoint> = resp
        .json()
        .await
        .context_internal("locationiq search parse failed")?;
    Ok(first_point(results))
}

// ponytail: confidence = "got a result"; tighten with place_rank/importance
// thresholds if vague matches slip through.
fn first_point(results: Vec<IqPoint>) -> Option<(f64, f64)> {
    let hit = results.into_iter().next()?;
    let lng = hit.lon.parse().ok()?;
    let lat = hit.lat.parse().ok()?;
    Some((lng, lat))
}

#[cfg(test)]
mod tests {
    use super::{IqPoint, first_point};

    #[test]
    fn parses_search_hit_to_lng_lat() {
        let body = r#"[{"lat":"50.85","lon":"4.35","display_name":"Brussels"}]"#;
        let results: Vec<IqPoint> = serde_json::from_str(body).unwrap();
        assert_eq!(first_point(results), Some((4.35, 50.85)));
    }

    #[test]
    fn empty_search_is_none() {
        let results: Vec<IqPoint> = serde_json::from_str("[]").unwrap();
        assert_eq!(first_point(results), None);
    }
}
