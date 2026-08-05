//! Required-only environment access.
//!
//! Every variable a service reads is mandatory and is declared in that service's
//! `.env`. There are deliberately no defaults here: a default is a value nobody
//! can see from the `.env` file, and the ones that used to exist
//! (`SURREALDB_USER=root`, `NATS_URL=nats://localhost:4222`, a 15-minute snapshot
//! interval) were invisible in exactly the deployments where being wrong costs
//! the most.
//!
//! These helpers exist only so the four `Config` structs don't each repeat the
//! same panic message. They read no variable of their own — the caller names
//! everything.

use std::{fmt::Display, str::FromStr};

/// Reads a required variable, or dies naming it.
///
/// Panicking is the point. Called from a `LazyLock` that every `main` forces at
/// startup, so a missing variable stops the process before it can bind a port —
/// rather than surfacing as a 500 from one handler an hour later.
pub fn require(key: &str) -> String {
    match std::env::var(key) {
        Ok(value) => value,
        Err(_) => panic!("{key} must be set — see this service's .env, which lists every variable it reads"),
    }
}

/// Same, then parses. Reports the offending value, because `PORT=80O` is not a
/// typo you spot by being told only that `PORT` failed to parse.
pub fn require_parsed<T>(key: &str) -> T
where
    T: FromStr,
    T::Err: Display,
{
    let raw = require(key);
    match raw.parse() {
        Ok(value) => value,
        Err(e) => panic!("{key}={raw:?} is not a valid {}: {e}", std::any::type_name::<T>()),
    }
}
