use argon2::{
    Argon2, PasswordHash, PasswordHasher, PasswordVerifier,
    password_hash::{SaltString, rand_core::OsRng},
};
use shared::error::myerror::{MyError, MyResult};

/// Hashing happens here rather than in SurrealQL (`crypto::argon2::generate`)
/// because a projection must be deterministic: Argon2 generates a random salt,
/// so every replica applying the same `UserRegistered` event would store a
/// different hash. Hash once, on the write side, and put the result in the event.

pub fn hash(password: &str) -> MyResult<String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| MyError::Bus(format!("hash password: {e}")))
}

/// Constant-time verification against a stored PHC string. A malformed or
/// missing hash is a failed login, never an error the caller has to handle —
/// otherwise the response would distinguish "no such user" from "wrong password".
pub fn verify(phc: &str, password: &str) -> bool {
    PasswordHash::new(phc)
        .map(|parsed| {
            Argon2::default()
                .verify_password(password.as_bytes(), &parsed)
                .is_ok()
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_rejects() {
        let phc = hash("Sup3rSecret!x").unwrap();
        assert!(verify(&phc, "Sup3rSecret!x"));
        assert!(!verify(&phc, "wrong"));
        // Salted: the same password hashes differently every time, which is
        // exactly why this can't live in a projection.
        assert_ne!(phc, hash("Sup3rSecret!x").unwrap());
        // A corrupt stored hash is a failed login, not a panic.
        assert!(!verify("not-a-phc-string", "Sup3rSecret!x"));
    }
}
