use sha2::{Digest, Sha256};
use uuid::Uuid;

/// SHA-256 of a refresh token, hex-encoded.
///
/// The plaintext token never leaves this process: it goes to the client in a
/// cookie, and only this hash goes into the event log and the projection. Lookups
/// hash the presented token and compare.
///
/// SHA-256 rather than Argon2 deliberately — a refresh token is 122 bits of
/// random UUID, not a human-chosen password, so there is nothing to brute-force
/// and a slow KDF would only add latency to every refresh.
pub fn hash(token: &Uuid) -> String {
    hex::encode(Sha256::digest(token.to_string().as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_stable_and_distinct() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        assert_eq!(
            hash(&a),
            hash(&a),
            "same token must hash the same on every instance"
        );
        assert_ne!(hash(&a), hash(&b));
        assert_eq!(hash(&a).len(), 64, "hex-encoded sha256");
    }
}
