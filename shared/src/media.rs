//! Object keys for images that live in R2.
//!
//! Only media-service ever mints one, but the services that accept a key back
//! from a client have to be able to recognise it — so the shape is defined here
//! rather than in either of them.
//!
//! A key is stored bare (`spots/019f….jpeg`), never as a full URL. These strings
//! go into events, and an event outlives whatever hostname was serving images the
//! day it was written: the public `pub-….r2.dev` URL is rate-limited and
//! development-only, so a listing created today must not be pinned to it. The
//! frontend joins the key onto `VITE_MEDIA_BASE` at render time.

/// Spot listing photos.
pub const PREFIX_SPOTS: &str = "spots";
/// Profile pictures.
pub const PREFIX_AVATARS: &str = "avatars";

/// Whether `key` is one media-service could have minted under `prefix`.
///
/// This is a trust boundary, not a tidiness check. Keys come straight back from
/// the client on create and edit, land in an event, and are rendered as an
/// `<img src>` by every other user — so without this a host could point their
/// listing's photos at any object, or at a path that isn't an object at all.
///
/// Deliberately stricter than the old `/api/spot/uploads/` prefix check it
/// replaces: the name must be the exact `{32 hex}.{ext}` shape `presign` builds,
/// which rules out traversal (`..`), nested prefixes, and empty names as a side
/// effect of the shape rather than as three separate guards.
pub fn is_media_key(key: &str, prefix: &str) -> bool {
    let Some(name) = key
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

#[cfg(test)]
mod tests {
    use super::*;

    const REAL: &str = "spots/019fd9a1a3cb7d12b96249db33e2a909.jpeg";

    #[test]
    fn accepts_a_key_presign_would_mint() {
        assert!(is_media_key(REAL, PREFIX_SPOTS));
        assert!(is_media_key(
            "avatars/019fd9a1a3cb7d12b96249db33e2a909.webp",
            PREFIX_AVATARS
        ));
    }

    /// Each of these reached the event log under the old prefix-only check, or
    /// would have if the client asked.
    #[test]
    fn rejects_anything_else() {
        for bad in [
            // Wrong bucket area — an avatar is not a listing photo.
            "avatars/019fd9a1a3cb7d12b96249db33e2a909.jpeg",
            // Traversal. Shape alone rules it out; there is no separate guard.
            "spots/../../etc/passwd",
            "spots/..%2F..%2Fetc%2Fpasswd",
            // Nested prefix — one level only, so a key cannot be steered elsewhere.
            "spots/other/019fd9a1a3cb7d12b96249db33e2a909.jpeg",
            // Empty name, no extension, no prefix at all.
            "spots/",
            "spots/019fd9a1a3cb7d12b96249db33e2a909",
            "019fd9a1a3cb7d12b96249db33e2a909.jpeg",
            // Not a uuid.
            "spots/hello.jpeg",
            "spots/019fd9a1a3cb7d12b96249db33e2a90.jpeg",
            // An absolute URL, which is what this whole scheme exists to keep out
            // of the log.
            "https://pub-xxxx.r2.dev/spots/019fd9a1a3cb7d12b96249db33e2a909.jpeg",
            // The old scheme.
            "/api/spot/uploads/019fd9a1a3cb7d12b96249db33e2a909.jpeg",
            "",
        ] {
            assert!(!is_media_key(bad, PREFIX_SPOTS), "accepted {bad:?}");
        }
    }

    /// `strip_prefix(prefix)` alone would accept `spotsX/…`; the separate `/`
    /// strip is what stops a bucket area whose name merely starts with ours.
    #[test]
    fn prefix_must_end_at_a_slash() {
        assert!(!is_media_key(
            "spotsX/019fd9a1a3cb7d12b96249db33e2a909.jpeg",
            PREFIX_SPOTS
        ));
    }
}
