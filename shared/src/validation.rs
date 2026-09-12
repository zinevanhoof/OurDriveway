//! The one shape every hand-written `garde` rule in this crate has.
//!
//! Here rather than in `requests`, because the rules are no longer only there: a
//! value type that carries its own rules — see
//! [`crate::general_models::spot::Availability`] — needs the same helper, and
//! `general_models` reaching into `requests` for it would invert the dependency.

/// Hold this condition, or fail with this message.
///
/// Every message is written out at the call site rather than derived from a
/// built-in rule, because these reach a form field verbatim. `garde` 0.23 has no
/// message override on its built-ins — the parser's `message` arm is commented
/// out — so a built-in rule means garde's wording, not ours.
pub(crate) fn require(ok: bool, msg: &'static str) -> garde::Result {
    if ok {
        Ok(())
    } else {
        Err(garde::Error::new(msg))
    }
}
