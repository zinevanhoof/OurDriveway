//! Single-purpose tokens that travel in a link inside an email.
//!
//! Minted by notification-service at send time and verified by whichever service
//! owns the action — never stored anywhere. That is deliberate: `STREAM_USERS`
//! has no `max_age`, so a token published into an event would be a credential
//! sitting in the log forever, readable by every projector that ever replays it.
//!
//! Both sides live here so the claim shape and the validation rules cannot drift
//! between the minting service and the verifying one.

use chrono::Utc;
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    claims::jwt_claims::ISSUER,
    error::myerror::{MyError, MyResult},
};

/// What a token is allowed to do. One variant per email that carries a link.
///
/// Checked on verification, so a password-reset link cannot be replayed against
/// the email-verification endpoint even though both are signed with the same
/// key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Purpose {
    VerifyEmail,
    ResetPassword,
}

/// Deliberately **not** [`crate::claims::jwt_claims::JwtClaims`].
///
/// That struct is the access-token shape and carries no audience or purpose
/// field, so a token minted in its shape and signed with `JWT_SECRET` would sail
/// straight through `AuthedJwt` as a full session. Separation here is by key
/// first — `EMAIL_TOKEN_SECRET` is a different secret entirely — and by this
/// `purpose` claim second.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EmailTokenClaims {
    /// The user's uuid, hyphenated — the one spelling used everywhere else.
    pub sub: String,
    pub purpose: Purpose,
    pub iat: i64,
    pub exp: i64,
    pub iss: String,
    /// The aggregate version the account stood at when this was minted, for a
    /// purpose whose link must work exactly once.
    ///
    /// `ResetPassword` carries one and `VerifyEmail` does not, because the two
    /// want opposite things: verification is deliberately replayable (scanners
    /// prefetch it, and setting `true` twice is setting `true`), while a reset
    /// link that still worked after the password was set would be a replay hole.
    /// Binding the token to a version is what makes it single-use with nothing
    /// stored anywhere — the redeeming service bumps the row past it.
    ///
    /// `default` + `skip_serializing_if` so a verification token is byte-identical
    /// to what it was before this field existed, and one minted before it still
    /// decodes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ver: Option<i64>,
}

/// What a good token proved.
///
/// Not a bare `Uuid` any more: a purpose that binds to a version has to hand that
/// version back, and [`Self::version`] is a `Result` rather than the `Option`
/// behind it so a caller that needs one cannot `if let` past its absence and
/// accept an unbound token.
pub struct Verified {
    pub user_id: Uuid,
    /// Private on purpose — see [`Self::version`].
    version: Option<i64>,
}

impl Verified {
    /// The version claim, or the same 400 every other rejection gives.
    pub fn version(&self) -> MyResult<i64> {
        self.version.ok_or_else(invalid)
    }
}

/// Mints a token good for one purpose, for one user, for `ttl_secs`.
///
/// The clock starts here rather than at registration, so a re-sent link is
/// genuinely fresh instead of inheriting the expiry of the account.
///
/// `version` binds the token to the state the account was in — `Some` for a
/// purpose whose link must work once, `None` for one that may be replayed. See
/// [`EmailTokenClaims::ver`].
pub fn mint(
    secret: &str,
    user_id: &Uuid,
    purpose: Purpose,
    ttl_secs: i64,
    version: Option<i64>,
) -> MyResult<String> {
    let now = Utc::now().timestamp();
    let claims = EmailTokenClaims {
        sub: user_id.to_string(),
        purpose,
        iat: now,
        exp: now + ttl_secs,
        iss: ISSUER.to_string(),
        ver: version,
    };

    Ok(encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )?)
}

/// Verifies signature, issuer, expiry and purpose, and returns what the token
/// proved.
///
/// Every failure collapses to the same 400. The caller is an unauthenticated
/// endpoint reachable by anyone holding a link, so distinguishing "expired" from
/// "wrong purpose" from "bad signature" only tells an attacker which of their
/// guesses was closest.
pub fn verify(secret: &str, token: &str, expected: Purpose) -> MyResult<Verified> {
    let mut validation = Validation::default();
    validation.set_issuer(&[ISSUER]);

    let data = decode::<EmailTokenClaims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &validation,
    )
    .map_err(|_| invalid())?;

    if data.claims.purpose != expected {
        return Err(invalid());
    }

    Ok(Verified {
        user_id: Uuid::parse_str(&data.claims.sub).map_err(|_| invalid())?,
        version: data.claims.ver,
    })
}

/// The one rejection this module ever gives, public so a caller enforcing its own
/// rule on a verified token — the single-use check in user-service — refuses with
/// exactly this rather than a message of its own. A used link and a forged one
/// have to be indistinguishable, and two copies of these strings would drift.
pub fn invalid() -> MyError {
    MyError::api(
        axum::http::StatusCode::BAD_REQUEST,
        "Invalid link",
        "This link is invalid or has expired. Request a new one.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "email-token-secret";
    const DAY: i64 = 24 * 60 * 60;

    #[test]
    fn round_trips_the_user_it_was_minted_for() {
        let user_id = Uuid::now_v7();
        let token = mint(SECRET, &user_id, Purpose::VerifyEmail, DAY, None).unwrap();
        assert_eq!(
            verify(SECRET, &token, Purpose::VerifyEmail)
                .unwrap()
                .user_id,
            user_id
        );
    }

    #[test]
    fn carries_the_version_it_was_minted_with() {
        let token = mint(
            SECRET,
            &Uuid::now_v7(),
            Purpose::ResetPassword,
            DAY,
            Some(7),
        )
        .unwrap();
        let verified = verify(SECRET, &token, Purpose::ResetPassword).unwrap();
        assert_eq!(verified.version().unwrap(), 7);
    }

    /// The fail-open this shape exists to make unwritable.
    ///
    /// `Verified::version` is a `Result` rather than the `Option` behind it
    /// precisely so a single-use check cannot be skipped by a token that simply
    /// omits the claim — which is the one token an attacker can mint freely if
    /// the signing key ever leaks into a purpose that does not bind.
    #[test]
    fn a_token_with_no_version_claim_will_not_produce_one() {
        let token = mint(SECRET, &Uuid::now_v7(), Purpose::ResetPassword, DAY, None).unwrap();
        assert!(
            verify(SECRET, &token, Purpose::ResetPassword)
                .unwrap()
                .version()
                .is_err()
        );
    }

    /// The claim is inside the signature, not beside it — so a link cannot be
    /// edited to name the version the account happens to be at now.
    #[test]
    fn two_versions_of_the_same_link_are_different_tokens() {
        let user_id = Uuid::now_v7();
        let first = mint(SECRET, &user_id, Purpose::ResetPassword, DAY, Some(1)).unwrap();
        let second = mint(SECRET, &user_id, Purpose::ResetPassword, DAY, Some(2)).unwrap();

        assert_ne!(first, second);
        let read = |t: &str| {
            verify(SECRET, t, Purpose::ResetPassword)
                .unwrap()
                .version()
                .unwrap()
        };
        assert_eq!(read(&first), 1);
        assert_eq!(read(&second), 2);
    }

    /// The whole reason `Purpose` exists. Both links are signed with the same
    /// key, so without this check a password-reset link would verify an email —
    /// and once `ResetPassword` gains an endpoint, the reverse would be an
    /// account takeover.
    #[test]
    fn a_token_is_useless_for_a_purpose_it_was_not_minted_for() {
        let token = mint(
            SECRET,
            &Uuid::now_v7(),
            Purpose::ResetPassword,
            DAY,
            Some(1),
        )
        .unwrap();
        assert!(verify(SECRET, &token, Purpose::VerifyEmail).is_err());
    }

    #[test]
    fn rejects_a_token_signed_with_a_different_secret() {
        let token = mint(SECRET, &Uuid::now_v7(), Purpose::VerifyEmail, DAY, None).unwrap();
        assert!(verify("not-the-secret", &token, Purpose::VerifyEmail).is_err());
    }

    #[test]
    fn rejects_an_expired_token() {
        // Negative TTL puts `exp` in the past. jsonwebtoken's default leeway is
        // 60s, so this has to be comfortably older than that.
        let token = mint(SECRET, &Uuid::now_v7(), Purpose::VerifyEmail, -600, None).unwrap();
        assert!(verify(SECRET, &token, Purpose::VerifyEmail).is_err());
    }

    /// The security assertion this module exists for.
    ///
    /// `EMAIL_TOKEN_SECRET` and `JWT_SECRET` are two variables holding two
    /// different values, and every service reads both. The day somebody
    /// "simplifies" them into one, an email-verification link becomes a bearer
    /// token for the account it names — a link that is mailed in cleartext,
    /// prefetched by scanners, and sitting in the recipient's inbox forever.
    /// This fails at that moment instead.
    #[test]
    fn an_email_token_is_not_accepted_as_an_access_token() {
        use crate::claims::jwt_claims::JwtClaims;

        let jwt_secret = "access-token-secret";
        let token = mint(SECRET, &Uuid::now_v7(), Purpose::VerifyEmail, DAY, None).unwrap();

        let mut validation = Validation::default();
        validation.set_issuer(&[ISSUER]);
        validation.validate_nbf = true;

        // Exactly what `AuthedJwt::from_request_parts` does.
        let decoded = decode::<JwtClaims>(
            &token,
            &DecodingKey::from_secret(jwt_secret.as_bytes()),
            &validation,
        );

        assert!(
            decoded.is_err(),
            "an email token verified as an access token: the two secrets have been merged"
        );
    }
}
