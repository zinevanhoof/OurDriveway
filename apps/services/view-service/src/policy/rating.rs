//! A rating as a screen shows it: the average, or nothing.

/// The average of `count` ratings summing to `sum`, or `None` when nobody has rated.
///
/// `None` rather than `0.0`, because "no ratings" and "rated zero" are different
/// answers, and ratings start at 1 anyway.
pub fn average(sum: Option<i64>, count: i64) -> Option<f64> {
    (count > 0).then(|| sum.unwrap_or(0) as f64 / count as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_ratings_is_no_rating() {
        assert_eq!(average(None, 0), None);
    }

    #[test]
    fn some_ratings_are_their_average() {
        assert_eq!(average(Some(9), 2), Some(4.5));
        assert_eq!(average(Some(5), 1), Some(5.0));
    }
}
