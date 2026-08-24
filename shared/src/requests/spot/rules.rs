//! The two field rules both halves of the listing form share.
//!
//! No test of its own: `create` and `update` each exercise both, and
//! `update_applies_the_same_rules_as_create` is what would catch one of them
//! being wired into only one of the two.

use crate::validation::require;

pub(super) fn is_positive(value: &i64, _: &()) -> garde::Result {
    require(*value > 0, "Price per hour must be greater than 0")
}

/// A listing needs at least one photo, and every photo has to be one of ours.
///
/// Both halves are here rather than in the route because there is now a single
/// image list. It used to be split — kept URLs inside the JSON, new files as
/// multipart parts — so "at least one" could only be judged after merging them,
/// and lived in the handler as a hand-rolled 422.
///
/// The membership half is a trust boundary: these strings come straight back from
/// the client and land in an event that every renter renders as an `<img src>` —
/// and Stripe fetches server-side for a Checkout Session. The ORIGIN is what is
/// really being checked; see [`crate::media::is_media_url`].
pub(super) fn are_spot_images(images: &Vec<String>, _: &()) -> garde::Result {
    require(!images.is_empty(), "Add at least one photo.")?;
    require(
        images
            .iter()
            .all(|image| crate::media::is_media_url(image, crate::media::PREFIX_SPOTS)),
        "Unknown photo.",
    )
}
