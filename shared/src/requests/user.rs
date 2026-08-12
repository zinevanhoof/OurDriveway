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
#[serde(rename_all = "camelCase")]
pub struct SignupRequest {
    #[garde(skip)]
    pub first_name: String,
    #[garde(skip)]
    pub last_name: String,
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

/// The token out of a verification link.
///
/// No `garde` rule beyond presence: `shared::email_token::verify` checks the
/// signature, issuer, expiry and purpose, and a shape check here would only
/// reject some invalid tokens slightly earlier while implying the rest are fine.
#[derive(Deserialize, Validate)]
pub struct VerifyEmailRequest {
    #[garde(length(min = 1))]
    pub token: String,
}

/// "Send me that link again."
#[derive(Deserialize, Validate)]
pub struct ResendVerificationRequest {
    #[garde(email)]
    pub email: String,
}

/// The edit-profile form's whole state — every field is required, so a partial
/// payload can't mean the client silently deciding what "unchanged" is.
///
/// `current_password` is the exception: it is only required when `email` differs
/// from the stored one, and only the service knows the stored one, so that rule
/// lives there rather than in garde.
#[derive(Deserialize, Validate)]
#[serde(rename_all = "camelCase")]
pub struct UpdateProfileRequest {
    #[garde(length(min = 1, max = 32))]
    pub first_name: String,
    #[garde(length(min = 1, max = 32))]
    pub last_name: String,
    #[garde(email)]
    pub email: String,
    #[garde(inner(length(min = 1, max = 16)))]
    pub license_plates: Vec<String>,
    #[garde(skip)]
    pub current_password: Option<String>,
    /// A media key the browser just uploaded to R2, or `None` for "unchanged" —
    /// the one field here that isn't required, because the form only sends it when
    /// the user actually picked a new picture.
    ///
    /// `None` is not "clear": that matches `UserUpdated`'s semantics and the
    /// repository's `?? profile_picture` coalescing. There is no remove-picture UI,
    /// so there is nothing that needs to mean "clear".
    #[garde(inner(custom(is_avatar)))]
    pub profile_picture: Option<String>,
}

/// The picture has to be a key media-service minted under the avatar prefix.
///
/// Same trust boundary as a spot's photos: it comes straight back from the client
/// and is rendered as an `<img src>` anywhere this user appears — on their own
/// profile, in a spot's owner card, on a booking row.
fn is_avatar(key: &String, _: &()) -> garde::Result {
    require(
        crate::media::is_media_key(key, crate::media::PREFIX_AVATARS),
        "Unknown picture.",
    )
}

/// Mirrors the change-password zod schema. The new password gets the signup
/// rules; the current one is only checked for presence, like `LoginRequest` —
/// it is verified against the stored hash, and complexity rules on it would just
/// lock out anyone who registered before the rules changed.
#[derive(Deserialize, Validate)]
#[serde(rename_all = "camelCase")]
pub struct ChangePasswordRequest {
    #[garde(skip)]
    pub current_password: String,
    #[garde(
        length(min = 8, max = 32),
        custom(has_upper),
        custom(has_lower),
        custom(has_digit),
        custom(has_special)
    )]
    pub new_password: String,
}

fn has_upper(value: &str, _: &()) -> garde::Result {
    require(
        value.chars().any(|c| c.is_ascii_uppercase()),
        "Must contain at least one uppercase letter",
    )
}
fn has_lower(value: &str, _: &()) -> garde::Result {
    require(
        value.chars().any(|c| c.is_ascii_lowercase()),
        "Must contain at least one lowercase letter",
    )
}
fn has_digit(value: &str, _: &()) -> garde::Result {
    require(
        value.chars().any(|c| c.is_ascii_digit()),
        "Must contain at least one number",
    )
}
fn has_special(value: &str, _: &()) -> garde::Result {
    require(
        value.chars().any(|c| !c.is_ascii_alphanumeric()),
        "Must contain at least one special character",
    )
}
fn require(ok: bool, msg: &'static str) -> garde::Result {
    if ok {
        Ok(())
    } else {
        Err(garde::Error::new(msg))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signup_rules_match_zod() {
        // valid
        assert!(
            SignupRequest {
                first_name: "test".into(),
                last_name: "test".into(),
                email: "a@b.com".into(),
                password: "Str0ng!pw".into()
            }
            .validate()
            .is_ok()
        );
        // all-lowercase letters -> upper, digit and special each report separately
        let report = SignupRequest {
            first_name: "test".into(),
            last_name: "test".into(),
            email: "a@b.com".into(),
            password: "weakpassword".into(),
        }
        .validate()
        .unwrap_err();
        let password_errors = report
            .iter()
            .filter(|(path, _)| path.to_string() == "password")
            .count();
        assert_eq!(password_errors, 3);
        // too short -> length fails
        assert!(
            SignupRequest {
                first_name: "test".into(),
                last_name: "test".into(),
                email: "a@b.com".into(),
                password: "Aa1!".into()
            }
            .validate()
            .is_err()
        );
        // bad email
        assert!(
            SignupRequest {
                first_name: "test".into(),
                last_name: "test".into(),
                email: "nope".into(),
                password: "Str0ng!pw".into()
            }
            .validate()
            .is_err()
        );
    }

    /// The new password must be held to exactly the signup rules — the frontend
    /// imports one zod chain for both, so a drift here is a form that passes and
    /// a request that 422s.
    #[test]
    fn change_password_rules_match_signup() {
        let change = |pw: &str| ChangePasswordRequest {
            current_password: "whatever".into(),
            new_password: pw.into(),
        }
        .validate();
        let signup = |pw: &str| SignupRequest {
            first_name: "test".into(),
            last_name: "test".into(),
            email: "a@b.com".into(),
            password: pw.into(),
        }
        .validate();

        for pw in ["Str0ng!pw", "weakpassword", "Aa1!", "NOLOWER1!", "nodigit!!"] {
            assert_eq!(
                change(pw).is_ok(),
                signup(pw).is_ok(),
                "{pw:?} judged differently by the two schemas"
            );
        }
        // The current password is never held to them: an old account whose
        // password predates the rules must still be able to change it.
        assert!(
            ChangePasswordRequest {
                current_password: "old".into(),
                new_password: "Str0ng!pw".into()
            }
            .validate()
            .is_ok()
        );
    }

    #[test]
    fn profile_rejects_blank_names_and_plates() {
        let profile = |plates: Vec<&str>| UpdateProfileRequest {
            first_name: "Zine".into(),
            last_name: "Van Hoof".into(),
            email: "a@b.com".into(),
            license_plates: plates.into_iter().map(Into::into).collect(),
            current_password: None,
            profile_picture: None,
        }
        .validate();

        assert!(profile(vec![]).is_ok(), "no plates is a valid profile");
        assert!(profile(vec!["1-ABC-123"]).is_ok());
        // An empty row is the "Add" button pressed and never filled in.
        assert!(profile(vec![""]).is_err());
        assert!(profile(vec!["THIS-PLATE-IS-FAR-TOO-LONG"]).is_err());

        assert!(
            UpdateProfileRequest {
                first_name: String::new(),
                last_name: "Van Hoof".into(),
                email: "a@b.com".into(),
                license_plates: vec![],
                current_password: None,
                profile_picture: None,
            }
            .validate()
            .is_err()
        );
    }

    /// The picture is client-supplied and rendered as an `<img src>` wherever this
    /// user appears, so only keys media-service minted get through. `None` has to
    /// stay valid — it is what the form sends whenever the picture is unchanged,
    /// which is most saves.
    #[test]
    fn profile_picture_must_be_our_own_media_key() {
        let with = |picture: Option<&str>| {
            UpdateProfileRequest {
                first_name: "Zine".into(),
                last_name: "Van Hoof".into(),
                email: "a@b.com".into(),
                license_plates: vec![],
                current_password: None,
                profile_picture: picture.map(Into::into),
            }
            .validate()
            .is_ok()
        };

        assert!(with(None), "unchanged is the common case");
        assert!(with(Some("avatars/019fd9a1a3cb7d12b96249db33e2a909.jpeg")));
        // A spot photo is not an avatar — the prefixes are not interchangeable.
        assert!(!with(Some("spots/019fd9a1a3cb7d12b96249db33e2a909.jpeg")));
        assert!(!with(Some("https://evil.example/track.png")));
        assert!(!with(Some("")));
    }

    #[test]
    fn login_only_validates_email() {
        assert!(
            LoginRequest {
                email: "a@b.com".into(),
                password: "x".into()
            }
            .validate()
            .is_ok()
        );
        assert!(
            LoginRequest {
                email: "nope".into(),
                password: "x".into()
            }
            .validate()
            .is_err()
        );
    }
}

