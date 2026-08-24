//! user-service's tables: who a person is, and whether they are still logged in.
//!
//! Private to that service — no browser identity can authenticate against this
//! database at all. The world-readable half of a user lives in [`super::view`].

pub mod refresh_token;
pub mod user;

pub use refresh_token::{RefreshToken, RefreshTokenPatch};
pub use user::{User, UserPatch};
