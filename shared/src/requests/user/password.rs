//! Forgetting a password and setting a new one from a mailed link.
//!
//! One file for both, the same reason `email.rs` holds two: they are the two ends
//! of one flow, and neither is big enough to be worth finding on its own.
//!
//! Nothing here is the change-password form — that is `update_user.rs`, behind a
//! session and a `current_password`. These two are unauthenticated by definition:
//! someone who has forgotten their password has neither.

use garde::Validate;
use serde::Deserialize;

use super::email::not_blank;
use super::fields::{Email, Password};

/// "I've forgotten it."
///
/// Answered 204 whether or not the address has an account — see
/// `UserService::forgot_password`. Nothing about this body's validation may leak
/// that either: a malformed address is a 422 because it is malformed, never
/// because it is unknown.
#[derive(Deserialize, Validate)]
pub struct ForgotPasswordRequest {
    #[garde(dive)]
    pub email: Email,
}

/// The token out of a reset link, and what to set.
///
/// No `current_password`: the token is the credential, and someone who has
/// forgotten their password cannot supply the old one by definition. The token
/// itself gets no rule beyond presence, for the reason spelled out on
/// [`super::VerifyEmailRequest`] — `shared::email_token::verify` is the authority.
///
/// [`Password`] rather than a bare `String`, so a reset is held to exactly the
/// rules signup is. The bare `String` on `LoginRequest` and `current_password` is
/// for reading a password that already exists; this writes a new one.
#[derive(Deserialize, Validate)]
pub struct ResetPasswordRequest {
    #[garde(custom(not_blank))]
    pub token: String,
    #[garde(dive)]
    pub password: Password,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forgot_wants_an_email() {
        let forgot = |email: &str| ForgotPasswordRequest {
            email: serde_json::from_value(email.into()).unwrap(),
        };
        assert!(forgot("a@b.com").validate().is_ok());
        assert!(forgot("nope").validate().is_err());
    }

    fn reset(token: &str, password: &str) -> ResetPasswordRequest {
        ResetPasswordRequest {
            token: token.into(),
            password: serde_json::from_value(password.into()).unwrap(),
        }
    }

    #[test]
    fn reset_only_rejects_an_empty_token() {
        assert!(reset("anything", "Str0ng!pass").validate().is_ok());
        assert!(reset("", "Str0ng!pass").validate().is_err());
    }

    /// The rule this file exists to not get wrong. A reset writes a password the
    /// same way signup does, so it is held to the same five rules — a `String`
    /// here would be a quiet back door around every one of them.
    #[test]
    fn reset_holds_the_new_password_to_the_signup_rules() {
        assert!(reset("token", "weak").validate().is_err());
        assert!(reset("token", "Str0ng!pass").validate().is_ok());
    }
}
