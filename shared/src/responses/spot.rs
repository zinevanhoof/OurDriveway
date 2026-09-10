use serde::Serialize;

/// One type-ahead suggestion from `GET /api/spot/address/suggest`.
///
/// A suggestion, **not an address of record.** The six text fields are what the create
/// form fills itself in with; what it submits is validated and re-geocoded server-side,
/// so nothing here is trusted on the way back in.
///
/// `lat`/`lng` are the exception and belong to a different caller: the map's search box
/// pans to the picked point (`LocationMap.vue`), while the create form deliberately drops
/// them — `CreateSpotRequest`'s address has no such field, because `SpotService::create_spot`
/// geocodes the submitted address itself rather than believing a client's coordinates.
///
/// Which is why they are `Option`: a hit whose coordinates are missing or unparseable is
/// still a perfectly good address to fill a form with, so it comes back without a point
/// instead of with `0, 0` — an island in the Gulf of Guinea that the map would happily
/// fly to.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutocompleteAddressResponse {
    pub line1: String,
    pub line2: Option<String>,
    pub city: String,
    pub postal_code: String,
    pub region: Option<String>,
    pub country: String,
    /// The whole address on one line, assembled from the fields above rather than taken
    /// from the provider's `display_name`. It is what the search box shows and what the
    /// create form submits for re-geocoding, so the two cannot disagree.
    pub formatted: String,
    pub lat: Option<f64>,
    pub lng: Option<f64>,
}
