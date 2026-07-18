use garde::Validate;
use serde::Deserialize;

/// Mirrors the login zod schema: valid email, password only checked for presence.
#[derive(Deserialize, Validate)]
pub struct LoginRequest {
    #[garde(email)]
    pub email: String,
    #[garde(skip)]
    pub password: String,
}

/// Mirrors the signup zod schema: valid email + password complexity.
#[derive(Deserialize, Validate)]
pub struct SignupRequest {
    #[garde(email)]
    pub email: String,
    #[garde(
        length(min = 8, max = 32),
        custom(has_upper),
        custom(has_lower),
        custom(has_digit),
        custom(has_special)
    )]
    pub password: String,
}

fn has_upper(value: &str, _: &()) -> garde::Result {
    require(value.chars().any(|c| c.is_ascii_uppercase()), "Must contain at least one uppercase letter")
}
fn has_lower(value: &str, _: &()) -> garde::Result {
    require(value.chars().any(|c| c.is_ascii_lowercase()), "Must contain at least one lowercase letter")
}
fn has_digit(value: &str, _: &()) -> garde::Result {
    require(value.chars().any(|c| c.is_ascii_digit()), "Must contain at least one number")
}
fn has_special(value: &str, _: &()) -> garde::Result {
    require(value.chars().any(|c| !c.is_ascii_alphanumeric()), "Must contain at least one special character")
}
fn require(ok: bool, msg: &'static str) -> garde::Result {
    if ok { Ok(()) } else { Err(garde::Error::new(msg)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signup_rules_match_zod() {
        // valid
        assert!(SignupRequest { email: "a@b.com".into(), password: "Str0ng!pw".into() }.validate().is_ok());
        // all-lowercase letters -> upper, digit and special each report separately
        let report = SignupRequest { email: "a@b.com".into(), password: "weakpassword".into() }.validate().unwrap_err();
        let password_errors = report.iter().filter(|(path, _)| path.to_string() == "password").count();
        assert_eq!(password_errors, 3);
        // too short -> length fails
        assert!(SignupRequest { email: "a@b.com".into(), password: "Aa1!".into() }.validate().is_err());
        // bad email
        assert!(SignupRequest { email: "nope".into(), password: "Str0ng!pw".into() }.validate().is_err());
    }

    #[test]
    fn login_only_validates_email() {
        assert!(LoginRequest { email: "a@b.com".into(), password: "x".into() }.validate().is_ok());
        assert!(LoginRequest { email: "nope".into(), password: "x".into() }.validate().is_err());
    }
}
