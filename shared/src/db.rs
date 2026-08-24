use std::borrow::Cow;

use chrono::{DateTime, Utc};
use surrealdb::{
    Surreal,
    engine::remote::ws::{Client, Ws},
    method::{Query, Transaction},
    opt::auth::Root,
    types::{Datetime, vars},
};

use crate::error::myerror::MyResult;

/// Opens the service's own connection to its own database.
///
/// One connection per process, authenticated once at boot with a database-level
/// user. This replaced the old per-request `db.authenticate(client_jwt)` in
/// `AuthedDb`, where every request re-authenticated the single shared socket —
/// so two concurrent requests could each be running under the other's identity.
///
/// Because this identity has a role rather than a record, table-level
/// `PERMISSIONS` do not constrain it. Authorization is the service's job now:
/// verify the JWT with `AuthedJwt`, then check ownership in Rust.
///
/// Credentials come from the caller's `Config`, not from this crate reading the
/// environment behind its back — the old `SURREALDB_USER` default of "root" was
/// a value no `.env` file mentioned.
pub async fn connect(addr: &str, username: &str, password: &str) -> MyResult<Surreal<Client>> {
    let db = Surreal::new::<Ws>(addr).await?;

    db.signin(Root {
        username: username.to_string(),
        password: password.to_string(),
    })
    .await?;

    db.use_ns("main").use_db("main").await?;

    Ok(db)
}

/// Anything a query can be issued against.
///
/// The point is that a generated model method does not care whether it is running
/// standalone or as one statement inside an open transaction — `Surreal<Client>`
/// and `Transaction<Client>` both satisfy this, so the same `User::patch(…)` call
/// works in either position.
pub trait Querier {
    fn q<'a>(&'a self, sql: impl Into<Cow<'a, str>>) -> Query<'a, Client>;
}

impl Querier for Surreal<Client> {
    fn q<'a>(&'a self, sql: impl Into<Cow<'a, str>>) -> Query<'a, Client> {
        self.query(sql)
    }
}

impl Querier for Transaction<Client> {
    fn q<'a>(&'a self, sql: impl Into<Cow<'a, str>>) -> Query<'a, Client> {
        self.query(sql)
    }
}

/// So a repository can be built over a *borrowed* querier. This is what lets a
/// projector do `UserRepository { q: &tx }` per event.
impl<T: Querier + ?Sized> Querier for &T {
    fn q<'a>(&'a self, sql: impl Into<Cow<'a, str>>) -> Query<'a, Client> {
        (**self).q(sql)
    }
}

/// The shared-connection case, and the one every long-lived repository should
/// use.
///
/// `Surreal` is already `Arc<Inner>` internally, but its `Clone` is **not** a
/// refcount bump: it mints a new session id and has the engine replay every
/// `replayable()` command onto it — `Attach`, `Signin`, `Use`. So handing
/// `db.clone()` to five things costs five sessions and five root sign-ins over
/// the socket. `Arc<Surreal<Client>>` clones the pointer instead, and everything
/// shares one session.
///
/// Safe here precisely because nothing re-authenticates per request: the
/// connection signs in once at boot (see [`connect`]) and every query carries its
/// own `bind` parameters rather than session-level `SET`. A codebase that called
/// `use_ns` or `authenticate` per request would need the separate sessions.
impl<T: Querier + ?Sized> Querier for std::sync::Arc<T> {
    fn q<'a>(&'a self, sql: impl Into<Cow<'a, str>>) -> Query<'a, Client> {
        (**self).q(sql)
    }
}

/// How far a projection has consumed its stream.
///
/// Advancing this and writing the data it describes must happen in one
/// transaction — see `bus::projector`. Nothing here enforces that; `bus::Tx` does,
/// by owning both ends and never handing a `Cursor` to the code that applies an
/// event.
pub struct Cursor<'a> {
    pub stream: &'a str,
    /// The envelope's `occurred_at`, never a local clock: every replica must
    /// derive the same timestamp from the same event.
    pub at: DateTime<Utc>,
    pub seq: u64,
}

impl Cursor<'_> {
    /// Highest stream sequence already applied. 0 when the projection is empty.
    pub async fn last_seq(db: &Surreal<Client>, stream: &str) -> MyResult<u64> {
        let seq: Option<i64> = db
            .query("SELECT VALUE last_seq FROM ONLY type::record('_projection', $s)")
            .bind(("s", stream.to_string()))
            .await?
            .take(0)?;
        Ok(seq.unwrap_or(0).max(0) as u64)
    }

    /// Moves the cursor. For an event that changes no rows, this is the whole
    /// effect — skip it and the event replays forever.
    pub async fn bump(&self, db: &impl Querier) -> MyResult<()> {
        db.q("UPSERT type::record('_projection', $s) SET last_seq = $seq, updated_at = $at")
            .bind(vars! {
                s:   self.stream.to_string(),
                at:  Datetime::from(self.at),
                seq: self.seq as i64,
            })
            .await?
            .check()?;
        Ok(())
    }
}
