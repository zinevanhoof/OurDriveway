use garde::Validate;
use serde::Deserialize;

use super::fields::{Email, Password, name_length};

/// Mirrors the signup zod schema: valid email + password complexity.
#[derive(Deserialize, Validate)]
#[serde(rename_all = "camelCase")]
pub struct SignupRequest {
    #[garde(custom(name_length))]
    pub first_name: String,
    #[garde(custom(name_length))]
    pub last_name: String,
    #[garde(dive)]
    pub email: Email,
    #[garde(dive)]
    pub password: Password,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signup(email: &str, password: &str) -> SignupRequest {
        SignupRequest {
            first_name: "test".into(),
            last_name: "test".into(),
            email: serde_json::from_value(email.into()).unwrap(),
            password: serde_json::from_value(password.into()).unwrap(),
        }
    }

    #[test]
    fn signup_rules_match_zod() {
        assert!(signup("a@b.com", "Str0ng!pw").validate().is_ok());

        // all-lowercase letters -> upper, digit and special each report separately.
        // `transparent` is what keeps the path `password` rather than `password[0]`.
        let report = signup("a@b.com", "weakpassword").validate().unwrap_err();
        let password_errors = report
            .iter()
            .filter(|(path, _)| path.to_string() == "password")
            .count();
        assert_eq!(password_errors, 3);

        assert!(signup("a@b.com", "Aa1!").validate().is_err()); // too short
        assert!(signup("nope", "Str0ng!pw").validate().is_err()); // bad email
    }

    /// Names are held to the same rule the profile form applies. They used to be
    /// `#[garde(skip)]` here, so signup accepted an empty name that the edit screen
    /// would then refuse.
    #[test]
    fn names_cannot_be_blank() {
        let mut r = signup("a@b.com", "Str0ng!pw");
        r.first_name = String::new();
        assert!(r.validate().is_err());
    }
}
