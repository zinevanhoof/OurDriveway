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
    events::user::record_key,
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
    /// The user's record key (`record_key`, hyphen-free), matching how ids are
    /// spelled everywhere else in the system.
    pub sub: String,
    pub purpose: Purpose,
    pub iat: i64,
    pub exp: i64,
    pub iss: String,
}

/// Mints a token good for one purpose, for one user, for `ttl_secs`.
///
/// The clock starts here rather than at registration, so a re-sent link is
/// genuinely fresh instead of inheriting the expiry of the account.
pub fn mint(secret: &str, user_id: &Uuid, purpose: Purpose, ttl_secs: i64) -> MyResult<String> {
    let now = Utc::now().timestamp();
    let claims = EmailTokenClaims {
        sub: record_key(user_id),
        purpose,
        iat: now,
        exp: now + ttl_secs,
        iss: ISSUER.to_string(),
    };

    Ok(encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )?)
}

/// Verifies signature, issuer, expiry and purpose, and returns the subject.
///
/// Every failure collapses to the same 400. The caller is an unauthenticated
/// endpoint reachable by anyone holding a link, so distinguishing "expired" from
/// "wrong purpose" from "bad signature" only tells an attacker which of their
/// guesses was closest.
pub fn verify(secret: &str, token: &str, expected: Purpose) -> MyResult<Uuid> {
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

    Uuid::parse_str(&data.claims.sub).map_err(|_| invalid())
}

fn invalid() -> MyError {
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
        let token = mint(SECRET, &user_id, Purpose::VerifyEmail, DAY).unwrap();
        assert_eq!(
            verify(SECRET, &token, Purpose::VerifyEmail).unwrap(),
            user_id
        );
    }

    /// The whole reason `Purpose` exists. Both links are signed with the same
    /// key, so without this check a password-reset link would verify an email —
    /// and once `ResetPassword` gains an endpoint, the reverse would be an
    /// account takeover.
    #[test]
    fn a_token_is_useless_for_a_purpose_it_was_not_minted_for() {
        let token = mint(SECRET, &Uuid::now_v7(), Purpose::ResetPassword, DAY).unwrap();
        assert!(verify(SECRET, &token, Purpose::VerifyEmail).is_err());
    }

    #[test]
    fn rejects_a_token_signed_with_a_different_secret() {
        let token = mint(SECRET, &Uuid::now_v7(), Purpose::VerifyEmail, DAY).unwrap();
        assert!(verify("not-the-secret", &token, Purpose::VerifyEmail).is_err());
    }

    #[test]
    fn rejects_an_expired_token() {
        // Negative TTL puts `exp` in the past. jsonwebtoken's default leeway is
        // 60s, so this has to be comfortably older than that.
        let token = mint(SECRET, &Uuid::now_v7(), Purpose::VerifyEmail, -600).unwrap();
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
        let token = mint(SECRET, &Uuid::now_v7(), Purpose::VerifyEmail, DAY).unwrap();

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
