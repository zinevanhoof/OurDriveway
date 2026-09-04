//! Request bodies for user-service, one file per form.
//!
//! Split by what writes them, not by type: the two email-verification bodies share
//! a file because they are two buttons on the same flow, and [`update_user`] holds
//! both halves of the user's own record — the profile form and the password form —
//! because one request serves both. [`fields`] holds the values more than one of
//! them needs, each as a newtype that owns its own rules.
//!
//! Re-exported flat, so every caller still writes
//! `shared::requests::user::SignupRequest` and the split is invisible outside.

mod email;
mod fields;
mod login;
mod signup;
mod update_user;

pub use email::{ResendVerificationRequest, VerifyEmailRequest};
pub use fields::{Country, Email, Password};
pub use login::LoginRequest;
pub use signup::SignupRequest;
pub use update_user::UpdateUserRequest;
