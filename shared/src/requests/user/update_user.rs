use garde::Validate;
use serde::Deserialize;

use super::fields::{Country, Email, Password, name_length, plate_length};
use crate::validation::require;

/// The caller's own record — both forms that write it: the profile screen and the
/// change-password screen.
///
/// Every field is optional and `None` means "unchanged", never "clear". That is
/// what lets one request serve two screens: the profile form submits its whole
/// half, the password form submits nothing but `current_password` and
/// `new_password`.
///
/// **One half per request.** `UserService::update_user` publishes exactly one
/// event, and a password change is still its own event — so a body carrying both
/// halves is refused there rather than having one of them silently dropped.
///
/// `current_password` is the field they share: it is what proves the caller is the
/// account's owner and not a stolen access token. Required for a password change,
/// and for an email change; nothing else on the profile form is worth that much.
/// Which rule applies is the service's call, because only it knows the stored
/// address.
#[derive(Deserialize, Validate)]
#[serde(rename_all = "camelCase")]
pub struct UpdateUserRequest {
    #[garde(inner(custom(name_length)))]
    pub first_name: Option<String>,
    #[garde(inner(custom(name_length)))]
    pub last_name: Option<String>,
    #[garde(dive)]
    pub email: Option<Email>,
    #[garde(inner(inner(custom(plate_length))))]
    pub license_plates: Option<Vec<String>>,
    /// Where this person banks, ISO 3166-1 alpha-2.
    ///
    /// On the profile rather than at signup because most people never need it: it
    /// exists for hosts, and it is asked for once, before Stripe will open a
    /// connected account for them. See [`Country`].
    #[garde(dive)]
    pub country: Option<Country>,
    #[garde(skip)]
    pub current_password: Option<String>,
    /// Held to exactly the signup rules — see `new_password_rules_match_signup`.
    /// The *current* one is only checked for presence, like [`super::LoginRequest`]:
    /// complexity rules on it would lock out anyone who registered before the rules
    /// changed.
    #[garde(dive)]
    pub new_password: Option<Password>,
    /// The media URL the browser just uploaded to R2, or `None` for "unchanged" —
    /// the form only sends it when the user actually picked a new picture. The whole
    /// URL, not a bucket key: `is_avatar` below defers to `media::is_media_url`.
    ///
    /// `None` is not "clear": that matches `UserUpdated`'s semantics and the
    /// repository's `?? profile_picture` coalescing. There is no remove-picture UI,
    /// so there is nothing that needs to mean "clear".
    #[garde(inner(custom(is_avatar)))]
    pub profile_picture: Option<String>,
}

/// The picture has to be a URL media-service minted under the avatar prefix.
///
/// Same trust boundary as a spot's photos: it comes straight back from the client
/// and is rendered as an `<img src>` anywhere this user appears — on their own
/// profile, in a spot's owner card, on a booking row.
fn is_avatar(url: &String, _: &()) -> garde::Result {
    require(
        crate::media::is_media_url(url, crate::media::PREFIX_AVATARS),
        "Unknown picture.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::requests::user::SignupRequest;

    /// Every field absent is a valid request — it is what makes one body serve two
    /// screens. Each test below starts from here and sets only what it is about.
    fn empty() -> UpdateUserRequest {
        UpdateUserRequest {
            first_name: None,
            last_name: None,
            email: None,
            license_plates: None,
            country: None,
            current_password: None,
            new_password: None,
            profile_picture: None,
        }
    }

    /// Two letters, and stored in one spelling — a host whose country is `be` on one
    /// save and `BE` on the next must not read as two different countries, because
    /// Stripe fixes this value permanently at account creation.
    #[test]
    fn the_country_is_two_letters_and_uppercase() {
        let country = |code: &str| UpdateUserRequest {
            country: Some(serde_json::from_value(code.into()).unwrap()),
            ..empty()
        };

        assert!(country("BE").validate().is_ok());
        assert!(country("be").validate().is_ok(), "case is normalised, not refused");
        assert_eq!(
            country("be").country.unwrap().into_inner(),
            "BE",
            "one spelling reaches the database"
        );

        for bad in ["", "B", "BEL", "b3", "🇧🇪"] {
            assert!(country(bad).validate().is_err(), "{bad:?} is not a country");
        }
    }

    #[test]
    fn profile_rejects_blank_names_and_plates() {
        let profile = |plates: Vec<&str>| {
            UpdateUserRequest {
                first_name: Some("Zine".into()),
                last_name: Some("Van Hoof".into()),
                email: Some(serde_json::from_value("a@b.com".into()).unwrap()),
                license_plates: Some(plates.into_iter().map(Into::into).collect()),
                ..empty()
            }
            .validate()
        };

        assert!(profile(vec![]).is_ok(), "no plates is a valid profile");
        assert!(profile(vec!["1-ABC-123"]).is_ok());
        // An empty row is the "Add" button pressed and never filled in.
        assert!(profile(vec![""]).is_err());
        assert!(profile(vec!["THIS-PLATE-IS-FAR-TOO-LONG"]).is_err());

        assert!(
            UpdateUserRequest {
                first_name: Some(String::new()),
                ..empty()
            }
            .validate()
            .is_err()
        );
    }

    /// The password screen's whole body. If any profile field ever goes back to
    /// being required, this 422s and the screen cannot save at all.
    #[test]
    fn the_password_form_is_a_valid_body_on_its_own() {
        assert!(
            UpdateUserRequest {
                current_password: Some("whatever".into()),
                new_password: Some(serde_json::from_value("Str0ng!pw".into()).unwrap()),
                ..empty()
            }
            .validate()
            .is_ok()
        );
    }

    /// The new password must be held to exactly the signup rules — the frontend
    /// imports one zod chain for both, so a drift here is a form that passes and
    /// a request that 422s.
    ///
    /// Lives on this side rather than with `SignupRequest`: signup's rules are the
    /// definition, this is the one that could quietly stop matching them.
    #[test]
    fn new_password_rules_match_signup() {
        let change = |pw: &str| {
            UpdateUserRequest {
                current_password: Some("whatever".into()),
                new_password: Some(serde_json::from_value(pw.into()).unwrap()),
                ..empty()
            }
            .validate()
        };
        let signup = |pw: &str| {
            SignupRequest {
                first_name: "test".into(),
                last_name: "test".into(),
                email: serde_json::from_value("a@b.com".into()).unwrap(),
                password: serde_json::from_value(pw.into()).unwrap(),
            }
            .validate()
        };

        for pw in [
            "Str0ng!pw",
            "weakpassword",
            "Aa1!",
            "NOLOWER1!",
            "nodigit!!",
        ] {
            assert_eq!(
                change(pw).is_ok(),
                signup(pw).is_ok(),
                "{pw:?} judged differently by the two schemas"
            );
        }
        // The current password is never held to them: an old account whose
        // password predates the rules must still be able to change it.
        assert!(
            UpdateUserRequest {
                current_password: Some("old".into()),
                new_password: Some(serde_json::from_value("Str0ng!pw".into()).unwrap()),
                ..empty()
            }
            .validate()
            .is_ok()
        );
    }

    /// The picture is client-supplied and rendered as an `<img src>` wherever this
    /// user appears, so only keys media-service minted get through. `None` has to
    /// stay valid — it is what the form sends whenever the picture is unchanged,
    /// which is most saves.
    #[test]
    fn profile_picture_must_be_our_own_media_key() {
        let with = |picture: Option<&str>| {
            UpdateUserRequest {
                profile_picture: picture.map(Into::into),
                ..empty()
            }
            .validate()
            .is_ok()
        };

        crate::media::init_test_base();
        let name = "019fd9a1a3cb7d12b96249db33e2a909.jpeg";
        let avatar = crate::media::url_for(crate::media::PREFIX_AVATARS, name);

        assert!(with(None), "unchanged is the common case");
        assert!(with(Some(&avatar)));
        // A spot photo is not an avatar — the prefixes are not interchangeable.
        assert!(!with(Some(&crate::media::url_for(
            crate::media::PREFIX_SPOTS,
            name
        ))));
        // Right shape, wrong origin — the check this scheme exists for.
        assert!(!with(Some(&format!("https://evil.example/avatars/{name}"))));
        assert!(!with(Some("https://evil.example/track.png")));
        // A bare key, i.e. the scheme this replaced.
        assert!(!with(Some(&format!("avatars/{name}"))));
        assert!(!with(Some("")));
    }
}
