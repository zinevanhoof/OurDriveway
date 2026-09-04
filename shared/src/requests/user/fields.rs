//! The two values the user forms share, each as a newtype that owns its rules.
//!
//! `#[garde(transparent)]` is what makes this worth doing: the error path stays the
//! *parent* field name, so a bad address on signup still reports as `email` and not
//! `email[0]`, and `serverErrors.ts` needs no change.
//!
//! Deliberately **not** validating in `Deserialize`. These types are request-only
//! today, but the rule that a value type must not validate on deserialization is
//! one worth keeping uniform — see the note on
//! [`crate::general_models::spot::Availability`].

use garde::Validate;
use serde::Deserialize;

use crate::validation::require;

/// An address the caller typed. One definition, four forms: signup, login, resend
/// verification, and the profile edit.
#[derive(Clone, Debug, Deserialize, Validate)]
#[serde(transparent)]
#[garde(transparent)]
pub struct Email(#[garde(email)] String);

impl Email {
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub fn into_inner(self) -> String {
        self.0
    }
}

/// One direction only: `String -> Email` would be a constructor that skips the
/// rule, which is the whole reason the field is private.
impl From<Email> for String {
    fn from(v: Email) -> Self {
        v.0
    }
}

impl std::fmt::Display for Email {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A password being *set* — signup, and the change-password form.
///
/// Never used for a password being *checked*: an account whose password predates
/// these rules must still be able to log in and change it, so `LoginRequest` and
/// `current_password` take a bare `String`.
///
/// The five rules report separately, which is the whole reason they are five: a
/// renter gets every reason their password was refused, not the first.
#[derive(Clone, Debug, Deserialize, Validate)]
#[serde(transparent)]
#[garde(transparent)]
pub struct Password(
    #[garde(
        custom(has_length),
        custom(has_upper),
        custom(has_lower),
        custom(has_digit),
        custom(has_special)
    )]
    String,
);

impl Password {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Spelled out rather than `length(min = 8, max = 32)`, because garde 0.23 has no
/// message override on its built-ins and this string reaches the form field
/// verbatim. Same reason every other rule in this crate is a `custom`.
fn has_length(value: &str, _: &()) -> garde::Result {
    require(
        (8..=32).contains(&value.chars().count()),
        "Must be between 8 and 32 characters",
    )
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

/// Where the caller banks — ISO 3166-1 alpha-2, as Stripe wants it.
///
/// A newtype rather than a rule function because it has an invariant beyond its
/// length: it is stored and sent **uppercase**, so two spellings of Belgium cannot
/// end up in two rows. `parse` is the only way in, which is what makes that true.
///
/// The country is only ever asked for on the profile screen, and only because
/// Stripe demands it before a connected account can receive money — it is fixed at
/// account creation and cannot be changed afterwards, so a wrong one costs a host
/// their payouts. Hence a value type, and not a free-text field.
///
/// Deliberately not checked against a list of the countries Stripe supports. That
/// list changes on Stripe's schedule, and a stale copy here would refuse a country
/// that had just become valid; Stripe answers `country_unsupported` itself, and
/// that answer is always current.
#[derive(Clone, Debug, Deserialize, Validate)]
#[serde(transparent)]
#[garde(transparent)]
pub struct Country(#[garde(custom(is_alpha2))] String);

impl Country {
    /// Uppercased, so the stored value has one spelling.
    pub fn into_inner(self) -> String {
        self.0.to_ascii_uppercase()
    }
}

fn is_alpha2(value: &String, _: &()) -> garde::Result {
    require(
        value.len() == 2 && value.chars().all(|c| c.is_ascii_alphabetic()),
        "Must be a two-letter country code.",
    )
}

/// A person's name, and a plate. Not newtypes: each is used once per form and a
/// wrapper would buy nothing but a `.0`.
pub(super) fn name_length(value: &String, _: &()) -> garde::Result {
    require(
        (1..=32).contains(&value.chars().count()),
        "Must be between 1 and 32 characters",
    )
}

pub(super) fn plate_length(value: &String, _: &()) -> garde::Result {
    require(
        (1..=16).contains(&value.chars().count()),
        "Must be between 1 and 16 characters",
    )
}
