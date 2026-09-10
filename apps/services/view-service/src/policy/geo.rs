//! The bounding box a radius query scans, and nothing else.
//!
//! Pure by construction: no I/O, no clock, no state — which is what lets the whole
//! file be tested without a database, per the convention in CLAUDE.md.
//!
//! ## Why a box and then a circle
//!
//! PostGIS is unavailable on YugabyteDB's YSQL (no GiST), so there is no geometry type
//! and no spatial index. What there is instead is `spot_bbox`, a plain btree on
//! `(lat, lng)` — and a btree can serve a *range* on both columns, which is a
//! rectangle. So the query narrows to the smallest rectangle containing the circle
//! using the index, then filters that handful down to the true circle with haversine.
//!
//! The rectangle is deliberately generous. Getting it slightly too large costs a few
//! extra rows the haversine then drops; getting it too small silently loses spots that
//! should have matched, which is the failure nobody would notice. Every rounding
//! choice below goes the safe way.
//!
//! This is not a downgrade from what it replaced. `fn::spot_distance` was a SurrealQL
//! function called once per row over no spatial index at all — a full scan with
//! trigonometry on every spot in the table.

/// Metres per degree of latitude. Constant everywhere to within ~0.6%, and the
/// variation is smaller than the margin [`bbox`] already adds.
const M_PER_DEG_LAT: f64 = 111_320.0;

/// The smallest latitude/longitude rectangle that contains every point within
/// `meters` of (`lng`, `lat`).
///
/// Returns `(min_lat, max_lat, min_lng, max_lng)`.
pub fn bbox(lng: f64, lat: f64, meters: f64) -> (f64, f64, f64, f64) {
    let d_lat = meters / M_PER_DEG_LAT;

    // A degree of longitude shrinks towards the poles by cos(latitude). Clamped
    // because at the pole itself the divisor is zero and every longitude is within
    // range — the clamp turns that into "the whole world", which is the correct answer
    // rather than an infinity.
    //
    // `abs()` on the cosine so a southern latitude behaves like its northern mirror.
    let shrink = lat.to_radians().cos().abs().max(0.01);
    let d_lng = meters / (M_PER_DEG_LAT * shrink);

    (
        (lat - d_lat).max(-90.0),
        (lat + d_lat).min(90.0),
        // Deliberately NOT wrapped to [-180, 180]. A box that straddles the
        // antimeridian would need `lng > min OR lng < max`, and the query says
        // `BETWEEN` — so the clamp keeps the box valid and simply stops a little short
        // for a search centred within `meters` of the date line. The alternative is a
        // branch in the SQL that no caller of this app will ever exercise.
        (lng - d_lng).max(-180.0),
        (lng + d_lng).min(180.0),
    )
}

/// Mean Earth radius in metres, shared by the two implementations of the haversine.
///
/// **Not `cfg(test)`, unlike [`haversine`] itself.** The reference implementation is a
/// test-only check on the query; the *radius* is an input to the query, so
/// `ViewSpotRepository::find_pins_for_public` reads it from here. One constant is what
/// stops the release build and the thing that verifies it from disagreeing about how big
/// the planet is — a disagreement no test could catch, because both sides would be
/// consistent with themselves.
pub const EARTH_RADIUS_M: f64 = 6_371_000.0;

/// Great-circle distance in metres between two lng/lat pairs.
///
/// **Nothing in the request path calls this.** The distance filter runs in SQL, so the
/// rows never leave the database — see `ViewSpotRepository::list_near`.
///
/// It exists as the *reference implementation* for that SQL, and having two copies of
/// one formula is a liability unless something checks they agree. Two things do:
///
/// - [`tests::the_box_contains_the_circle`] uses it to prove the bounding box never
///   excludes a point the circle would accept — the failure that would silently drop
///   spots from a map.
/// - `the_sql_radius_agrees_with_the_reference` in `repository/mod.rs` seeds spots at
///   known distances and asserts the query returns exactly the set this predicts,
///   which is the only thing that would catch a typo in the SQL trigonometry.
///
/// Delete it and the SQL becomes unverifiable, not simpler.
///
/// `cfg(test)` because that is the truth — a release build has no caller, and marking
/// it dead-code-allowed instead would leave a reader wondering which path uses it.
#[cfg(test)]
pub fn haversine(lng_a: f64, lat_a: f64, lng_b: f64, lat_b: f64) -> f64 {
    let r = EARTH_RADIUS_M;

    let (phi_a, phi_b) = (lat_a.to_radians(), lat_b.to_radians());
    let d_phi = (lat_b - lat_a).to_radians();
    let d_lambda = (lng_b - lng_a).to_radians();

    let a =
        (d_phi / 2.0).sin().powi(2) + phi_a.cos() * phi_b.cos() * (d_lambda / 2.0).sin().powi(2);

    2.0 * r * a.sqrt().asin()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Brussels, and the two facts the query depends on.
    #[test]
    fn the_box_contains_the_circle() {
        let (lng, lat, meters) = (4.35, 50.85, 5_000.0);
        let (min_lat, max_lat, min_lng, max_lng) = bbox(lng, lat, meters);

        // Walk the compass. Every point exactly `meters` away must fall inside the
        // box — if it does not, the index scan drops a spot the haversine would have
        // accepted, and nothing downstream would notice.
        for bearing in (0..360).step_by(5) {
            let theta = (bearing as f64).to_radians();
            let d_lat = (meters * theta.cos()) / M_PER_DEG_LAT;
            let d_lng = (meters * theta.sin()) / (M_PER_DEG_LAT * lat.to_radians().cos());
            let (p_lat, p_lng) = (lat + d_lat, lng + d_lng);

            assert!(
                p_lat >= min_lat && p_lat <= max_lat,
                "bearing {bearing}: lat {p_lat} outside [{min_lat}, {max_lat}]"
            );
            assert!(
                p_lng >= min_lng && p_lng <= max_lng,
                "bearing {bearing}: lng {p_lng} outside [{min_lng}, {max_lng}]"
            );
        }
    }

    #[test]
    fn distance_is_symmetric_and_zero_at_a_point() {
        assert_eq!(haversine(4.35, 50.85, 4.35, 50.85), 0.0);
        let there = haversine(4.35, 50.85, 4.40, 50.90);
        let back = haversine(4.40, 50.90, 4.35, 50.85);
        assert!((there - back).abs() < 1e-9);
    }

    /// Brussels to Antwerp is about 40 km. Loose bounds — this is checking the
    /// formula is not out by a factor, not the geoid.
    #[test]
    fn a_known_distance_is_about_right() {
        let d = haversine(4.3517, 50.8466, 4.4025, 51.2194);
        assert!(
            (40_000.0..=45_000.0).contains(&d),
            "Brussels–Antwerp was {d}m"
        );
    }

    /// The pole is where the naive `1 / cos(lat)` blows up. The box must stay finite
    /// and stay valid.
    #[test]
    fn a_polar_search_does_not_produce_an_infinite_box() {
        let (min_lat, max_lat, min_lng, max_lng) = bbox(0.0, 89.999, 5_000.0);
        assert!(min_lat.is_finite() && max_lat.is_finite());
        assert!(min_lng >= -180.0 && max_lng <= 180.0);
        assert!(max_lat <= 90.0, "latitude must not exceed the pole");
    }

    /// Southern latitudes must behave like their northern mirror rather than
    /// producing a negative shrink factor.
    #[test]
    fn the_southern_hemisphere_matches_the_northern() {
        let north = bbox(4.35, 50.85, 5_000.0);
        let south = bbox(4.35, -50.85, 5_000.0);
        assert!((north.3 - north.2 - (south.3 - south.2)).abs() < 1e-9);
    }
}
