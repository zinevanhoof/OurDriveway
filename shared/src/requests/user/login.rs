use garde::Validate;
use serde::Deserialize;

use super::fields::Email;

/// Mirrors the login zod schema: valid email, password only checked for presence.
///
/// The password deliberately gets none of [`super::fields::Password`]'s rules: they
/// are verified against a stored hash, and holding an old account's password to
/// rules that postdate it would lock out exactly the people who need to log in and
/// change it. Hence `String` here and not the newtype.
#[derive(Deserialize, Validate)]
pub struct LoginRequest {
    #[garde(dive)]
    pub email: Email,
    #[garde(skip)]
    pub password: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn login(email: &str) -> LoginRequest {
        LoginRequest {
            email: serde_json::from_value(email.into()).unwrap(),
            password: "x".into(),
        }
    }

    #[test]
    fn login_only_validates_email() {
        assert!(login("a@b.com").validate().is_ok());
        assert!(login("nope").validate().is_err());
    }
}
