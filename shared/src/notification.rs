//! What the system can send, as data.
//!
//! One variant per template that exists. A worker's whole job is turning an
//! event into `Option<Mail>`; everything downstream — which template alias,
//! which variables — is decided once in notification-service's `template.rs`.
//!
//! Lives in `shared` rather than in notification-service so that a service which
//! *raises* a notification can be type-checked against the same vocabulary the
//! service that *sends* it uses.

/// Fields mirror the Resend template's variables one for one, and a field is
/// `Option` exactly when the template defines a default for it.
#[derive(Clone, Debug)]
pub enum Mail {
    VerifyEmail {
        to: String,
        /// The one variable the template cannot render without.
        verification_url: String,
        /// Template-side default exists. `None` omits the key from the payload
        /// entirely, which is what lets that default apply — sending `""` would
        /// override it with an empty string and render a greeting addressed to
        /// nobody.
        first_name: Option<String>,
        company_name: Option<String>,
    },
}

impl Mail {
    /// Who this is going to. Every variant has a recipient; only the body
    /// differs.
    pub fn to(&self) -> &str {
        match self {
            Mail::VerifyEmail { to, .. } => to,
        }
    }

    /// Stable label for logs and metrics. Deliberately not `Debug` — that would
    /// print the recipient and the token-bearing URL into the log.
    pub fn kind(&self) -> &'static str {
        match self {
            Mail::VerifyEmail { .. } => "verify_email",
        }
    }
}
