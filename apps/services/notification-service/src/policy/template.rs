//! What an email says, decided from the [`Mail`] alone.
//!
//! *Which* Resend template renders it is deliberately not here: that is one
//! `CONFIG` lookup per variant, so it sits in `client/mailer.rs` beside the other
//! provider-side identifiers and keeps this file testable with no environment.

use std::collections::HashMap;

use serde_json::Value;
use shared::notification::Mail;

/// The only place a template variable *name* is spelled.
pub fn variables(mail: &Mail) -> HashMap<String, Value> {
    match mail {
        Mail::VerifyEmail {
            verification_url,
            first_name,
            company_name,
            ..
        } => {
            let mut vars = HashMap::new();
            vars.insert("verification_url".to_string(), json(verification_url));
            // Inserted only when present. The template defines defaults for
            // these two, and a default applies when the key is *absent* — send
            // `""` and it wins over the default, greeting the reader by no name
            // at all.
            insert_opt(&mut vars, "first_name", first_name.as_deref());
            insert_opt(&mut vars, "company_name", company_name.as_deref());
            vars
        }

        Mail::ResetPassword {
            reset_url,
            first_name,
            company_name,
            ..
        } => {
            let mut vars = HashMap::new();
            vars.insert("reset_url".to_string(), json(reset_url));
            insert_opt(&mut vars, "first_name", first_name.as_deref());
            insert_opt(&mut vars, "company_name", company_name.as_deref());
            vars
        }
    }
}

/// Subject line per template.
///
/// Resend requires a subject even when a template supplies the body, so it
/// cannot live in the dashboard alongside the rest of the design.
pub fn subject(mail: &Mail) -> &'static str {
    match mail {
        Mail::VerifyEmail { .. } => "Verify your email address",
        Mail::ResetPassword { .. } => "Reset your password",
    }
}

fn insert_opt(vars: &mut HashMap<String, Value>, key: &str, value: Option<&str>) {
    if let Some(value) = value {
        vars.insert(key.to_string(), json(value));
    }
}

fn json(value: &str) -> Value {
    Value::String(value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mail(first_name: Option<&str>, company_name: Option<&str>) -> Mail {
        Mail::VerifyEmail {
            to: "renter@example.com".to_string(),
            verification_url: "https://ourdriveway.com/verify?token=abc".to_string(),
            first_name: first_name.map(str::to_string),
            company_name: company_name.map(str::to_string),
        }
    }

    #[test]
    fn always_supplies_the_one_variable_the_template_cannot_default() {
        let vars = variables(&mail(None, None));
        assert_eq!(
            vars.get("verification_url").and_then(Value::as_str),
            Some("https://ourdriveway.com/verify?token=abc")
        );
    }

    /// The distinction the whole `Option` exists for. An absent key lets the
    /// template's own default render; a present-but-empty key overrides it, and
    /// the reader gets "Hi ," — which is the bug this asserts against.
    #[test]
    fn an_absent_optional_is_omitted_rather_than_blanked() {
        let vars = variables(&mail(None, None));
        assert!(!vars.contains_key("first_name"), "{vars:?}");
        assert!(!vars.contains_key("company_name"), "{vars:?}");
    }

    #[test]
    fn a_supplied_optional_is_passed_through() {
        let vars = variables(&mail(Some("Zine"), Some("OurDriveway")));
        assert_eq!(vars.get("first_name").and_then(Value::as_str), Some("Zine"));
        assert_eq!(
            vars.get("company_name").and_then(Value::as_str),
            Some("OurDriveway")
        );
    }

    fn reset_mail(first_name: Option<&str>, company_name: Option<&str>) -> Mail {
        Mail::ResetPassword {
            to: "renter@example.com".to_string(),
            reset_url: "https://ourdriveway.com/reset-password?token=abc".to_string(),
            first_name: first_name.map(str::to_string),
            company_name: company_name.map(str::to_string),
        }
    }

    #[test]
    fn a_reset_mail_carries_its_url() {
        let vars = variables(&reset_mail(None, None));
        assert_eq!(
            vars.get("reset_url").and_then(Value::as_str),
            Some("https://ourdriveway.com/reset-password?token=abc")
        );
        // Not the other variant's key, which a copy-pasted arm would leave behind.
        assert!(!vars.contains_key("verification_url"), "{vars:?}");
    }

    /// Per variant, because the omit-rather-than-blank rule is written out per
    /// variant — the arm that forgets `insert_opt` compiles perfectly.
    #[test]
    fn an_absent_optional_is_omitted_on_a_reset_too() {
        let vars = variables(&reset_mail(None, None));
        assert!(!vars.contains_key("first_name"), "{vars:?}");
        assert!(!vars.contains_key("company_name"), "{vars:?}");
    }

    #[test]
    fn each_mail_has_its_own_subject() {
        assert_ne!(subject(&mail(None, None)), subject(&reset_mail(None, None)));
    }
}
