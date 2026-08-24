//! Which wall clock a spot runs on.
//!
//! The one thing every downstream deadline is measured against — booking-service's
//! cancel cutoff, the `ends_at` folded onto each booking — so it is decided once
//! here, at create and update, and travels in the event.

use std::sync::OnceLock;

use tzf_rs::DefaultFinder;

/// IANA timezone name for a point. `lng`/`lat` order (geo `Point`: x = lng,
/// y = lat). Falls back to "Etc/UTC" when the point matches no zone (e.g. open
/// ocean, or the hardcoded placeholder coords until geocoding is wired).
///
/// The `OnceLock` is a lookup table, not state: `DefaultFinder` is an immutable
/// index over the world's zone polygons, built once because building it is the
/// expensive part. Nothing about a call is remembered.
pub fn for_coords(lng: f64, lat: f64) -> String {
    static FINDER: OnceLock<DefaultFinder> = OnceLock::new();
    let name = FINDER.get_or_init(DefaultFinder::new).get_tz_name(lng, lat);
    if name.is_empty() {
        "Etc/UTC".to_string()
    } else {
        name.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::for_coords;

    #[test]
    fn derives_zone_from_coords() {
        // Brussels (lng, lat).
        assert_eq!(for_coords(4.35, 50.85), "Europe/Brussels");
        // Open ocean resolves to a nautical Etc/GMT zone (tzf-rs never returns
        // empty); the is_empty -> "Etc/UTC" guard stays as a defensive fallback.
        assert_eq!(for_coords(-40.0, 0.0), "Etc/GMT+3");
    }
}
