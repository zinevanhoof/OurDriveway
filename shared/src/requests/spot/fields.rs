//! The two listing values both halves of the form share, each owning its rules.
//!
//! `#[garde(transparent)]` keeps the error path at the parent field name, so a
//! short title still reports as `title` and the frontend mapping is unchanged.

use garde::Validate;
use serde::Deserialize;

use crate::validation::require;

#[derive(Clone, Debug, Deserialize, Validate)]
#[serde(transparent)]
#[garde(transparent)]
pub struct Title(#[garde(custom(title_length))] String);

#[derive(Clone, Debug, Deserialize, Validate)]
#[serde(transparent)]
#[garde(transparent)]
pub struct Description(#[garde(custom(description_length))] String);

impl Title {
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl Description {
    pub fn into_inner(self) -> String {
        self.0
    }
}

/// So a call site can `.map(Into::into)` an `Option` of one without a closure.
/// One direction only: `String -> Title` would be a constructor that skips the rule.
impl From<Title> for String {
    fn from(v: Title) -> Self {
        v.0
    }
}

impl From<Description> for String {
    fn from(v: Description) -> Self {
        v.0
    }
}

/// Spelled out rather than `length(min = 5, max = 32)`: garde 0.23 has no message
/// override on its built-ins, and this reaches the form field verbatim.
fn title_length(value: &str, _: &()) -> garde::Result {
    require(
        (5..=32).contains(&value.chars().count()),
        "Must be between 5 and 32 characters",
    )
}

fn description_length(value: &str, _: &()) -> garde::Result {
    require(
        (20..=100).contains(&value.chars().count()),
        "Must be between 20 and 100 characters",
    )
}
