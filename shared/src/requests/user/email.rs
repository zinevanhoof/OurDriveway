//! Proving an address is reachable: the link, and asking for it again.
//!
//! One file for both because they are one flow — the resend button exists only
//! for people whose verify link never arrived — and neither is big enough to be
//! worth finding on its own.

use garde::Validate;
use serde::Deserialize;

use super::fields::Email;
use crate::validation::require;

/// The token out of a verification link.
///
/// No `garde` rule beyond presence: `shared::email_token::verify` checks the
/// signature, issuer, expiry and purpose, and a shape check here would only
/// reject some invalid tokens slightly earlier while implying the rest are fine.
#[derive(Deserialize, Validate)]
pub struct VerifyEmailRequest {
    #[garde(custom(not_blank))]
    pub token: String,
}

/// Spelled out rather than `length(min = 1)` — garde 0.23 cannot override a
/// built-in's message, and these strings reach the client verbatim.
pub(crate) fn not_blank(value: &String, _: &()) -> garde::Result {
    require(!value.trim().is_empty(), "Required.")
}

/// "Send me that link again."
#[derive(Deserialize, Validate)]
pub struct ResendVerificationRequest {
    #[garde(dive)]
    pub email: Email,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only that the empty body is refused. Anything stricter would be this file
    /// guessing at a format `email_token::verify` is the authority on.
    #[test]
    fn verify_only_rejects_an_empty_token() {
        assert!(
            VerifyEmailRequest {
                token: "anything".into()
            }
            .validate()
            .is_ok()
        );
        assert!(
            VerifyEmailRequest {
                token: String::new()
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn resend_wants_an_email() {
        let resend = |email: &str| ResendVerificationRequest {
            email: serde_json::from_value(email.into()).unwrap(),
        };
        assert!(resend("a@b.com").validate().is_ok());
        assert!(resend("nope").validate().is_err());
    }
}
