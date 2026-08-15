//! Absolute URLs for images that live in R2.
//!
//! media-service mints one per upload; the services that accept it back from a
//! client have to recognise it, so the shape lives here rather than in either.
//!
//! A stored value is the whole URL (`https://images.example.com/spots/019f….jpeg`),
//! not a bare key. That means the hostname is written into events, which is a real
//! trade: an event log is permanent, so the day images move to another origin every
//! stored URL points at the old one and only a rewrite fixes it. It is accepted
//! deliberately — each environment serves images from a domain it owns, so nothing
//! is pinned to a provider's throwaway hostname — and it buys back the thing that
//! made keys awkward: a value that is already loadable everywhere it is read, with
//! no service left holding a hostname purely to hand one to Stripe.
//!
//! **The origin is the trust boundary.** These strings come straight back from the
//! client, land in an event, and are rendered as an `<img src>` by every other user
//! — and Stripe fetches them server-side for a Checkout Session. Accepting an
//! arbitrary absolute URL would let a host point a listing's photos at any host on
//! the internet, so [`is_media_url`] pins them to [`init_base`]'s value and is not
//! optional anywhere a client-supplied image arrives.

use std::sync::OnceLock;

/// Spot listing photos.
pub const PREFIX_SPOTS: &str = "spots";
/// Profile pictures.
pub const PREFIX_AVATARS: &str = "avatars";

/// Where images are served from, e.g. `https://images.ourdriveway.com`.
///
/// A process-wide `OnceLock` rather than a parameter because the validators are
/// `garde` custom functions on the request structs, which take no state — the same
/// reason and the same shape as `shared::init_jwt_decoding_key`.
static BASE: OnceLock<String> = OnceLock::new();

/// Installs the origin. Call once from `main`, beside `LazyLock::force(&CONFIG)`,
/// in every service that either mints or validates an image URL.
///
/// A repeat call is ignored rather than fatal, like `init_jwt_decoding_key`: the
/// value comes from a `LazyLock` that resolves once, so a second call can only be
/// carrying the same base.
pub fn init_base(base: &str) {
    let _ = BASE.set(normalise(base));
}

/// Trailing slash off, so a base configured either way mints identical URLs — and
/// one deployment's images still validate under the next. Split out from
/// [`init_base`] because `BASE` is set once per process, which makes the trim
/// untestable through the setter.
fn normalise(base: &str) -> String {
    base.trim_end_matches('/').to_string()
}

/// Panics if `main` never installed the base.
///
/// Unreachable in practice, and loud on purpose: the alternative is a validator
/// that silently rejects every image, or worse, one that accepts every origin.
fn base() -> &'static str {
    BASE.get()
        .map(String::as_str)
        .expect("media::init_base must be called from main before serving requests")
}

/// The URL media-service hands back for a freshly presigned object.
pub fn url_for(prefix: &str, name: &str) -> String {
    format!("{}/{prefix}/{name}", base())
}

/// Whether `url` is one media-service could have minted under `prefix`.
///
/// Two things are checked and both matter: the origin is ours, and the path is the
/// exact `{prefix}/{32 hex}.{ext}` shape [`url_for`] builds. The shape half rules
/// out traversal (`..`), nested prefixes and empty names as a consequence of being
/// exact rather than as three separate guards.
pub fn is_media_url(url: &str, prefix: &str) -> bool {
    // The `/` is stripped separately so a base of `https://img.example` cannot be
    // satisfied by `https://img.example.evil.com/…` — the same boundary reasoning
    // as the prefix check below.
    let Some(path) = url
        .strip_prefix(base())
        .and_then(|rest| rest.strip_prefix('/'))
    else {
        return false;
    };

    let Some(name) = path
        .strip_prefix(prefix)
        .and_then(|rest| rest.strip_prefix('/'))
    else {
        return false;
    };

    let Some((uuid, ext)) = name.split_once('.') else {
        return false;
    };

    uuid.len() == 32
        && uuid.chars().all(|c| c.is_ascii_hexdigit())
        && (1..=5).contains(&ext.len())
        && ext.chars().all(|c| c.is_ascii_alphanumeric())
}

/// The origin every test in this crate installs. `BASE` is process-wide and set
/// once, so the media tests and the request-validator tests have to agree on it —
/// whichever runs first wins and the rest are no-ops.
#[cfg(test)]
pub(crate) const TEST_BASE: &str = "https://images.example.com";

/// Installs [`TEST_BASE`]. Idempotent, so every test can open with it.
#[cfg(test)]
pub(crate) fn init_test_base() {
    init_base(TEST_BASE);
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE_URL: &str = TEST_BASE;
    const NAME: &str = "019fd9a1a3cb7d12b96249db33e2a909.jpeg";

    /// `BASE` is process-wide and set once, so every test in this binary has to
    /// agree on it. First call wins; the rest are no-ops.
    fn init() {
        init_test_base();
    }

    fn spot_url() -> String {
        format!("{BASE_URL}/{PREFIX_SPOTS}/{NAME}")
    }

    #[test]
    fn accepts_a_url_media_service_would_mint() {
        init();
        assert!(is_media_url(&spot_url(), PREFIX_SPOTS));
        assert!(is_media_url(
            &format!("{BASE_URL}/{PREFIX_AVATARS}/{NAME}"),
            PREFIX_AVATARS
        ));
    }

    #[test]
    fn url_for_round_trips_through_the_validator() {
        init();
        assert!(is_media_url(&url_for(PREFIX_SPOTS, NAME), PREFIX_SPOTS));
        assert_eq!(url_for(PREFIX_SPOTS, NAME), spot_url());
    }

    /// Tests `normalise` directly, NOT through `init_base`: `BASE` is a process-wide
    /// `OnceLock`, so whichever test ran first already set it and a second call is a
    /// no-op — this assertion would pass without exercising the trim at all.
    #[test]
    fn base_is_normalised() {
        assert_eq!(normalise("https://images.example.com/"), BASE_URL);
        assert_eq!(normalise("https://images.example.com"), BASE_URL);
    }

    /// The half that is a security control rather than tidiness.
    #[test]
    fn rejects_a_foreign_origin() {
        init();
        for bad in [
            // Somebody else's host entirely — the whole reason this check exists.
            &format!("https://evil.example.com/{PREFIX_SPOTS}/{NAME}"),
            // Prefix-of-a-hostname: passes a naive `starts_with(base)`, which is
            // why the `/` is stripped as a separate step.
            &format!("https://images.example.com.evil.com/{PREFIX_SPOTS}/{NAME}"),
            // Right host, wrong scheme.
            &format!("http://images.example.com/{PREFIX_SPOTS}/{NAME}"),
            // A bare key, i.e. the scheme this replaced.
            &format!("{PREFIX_SPOTS}/{NAME}"),
        ] {
            assert!(!is_media_url(bad, PREFIX_SPOTS), "accepted {bad:?}");
        }
    }

    #[test]
    fn rejects_anything_but_the_exact_shape() {
        init();
        for bad in [
            // Wrong bucket area — an avatar is not a listing photo.
            format!("{BASE_URL}/{PREFIX_AVATARS}/{NAME}"),
            // Traversal. Shape alone rules it out; there is no separate guard.
            format!("{BASE_URL}/{PREFIX_SPOTS}/../../etc/passwd"),
            format!("{BASE_URL}/{PREFIX_SPOTS}/..%2F..%2Fetc%2Fpasswd"),
            // Nested prefix — one level only, so a key cannot be steered elsewhere.
            format!("{BASE_URL}/{PREFIX_SPOTS}/other/{NAME}"),
            // Empty name, no extension.
            format!("{BASE_URL}/{PREFIX_SPOTS}/"),
            format!("{BASE_URL}/{PREFIX_SPOTS}/019fd9a1a3cb7d12b96249db33e2a909"),
            // Not a uuid.
            format!("{BASE_URL}/{PREFIX_SPOTS}/hello.jpeg"),
            format!("{BASE_URL}/{PREFIX_SPOTS}/019fd9a1a3cb7d12b96249db33e2a90.jpeg"),
            // A bucket area whose name merely starts with ours.
            format!("{BASE_URL}/{PREFIX_SPOTS}X/{NAME}"),
            String::new(),
        ] {
            assert!(!is_media_url(&bad, PREFIX_SPOTS), "accepted {bad:?}");
        }
    }
}
