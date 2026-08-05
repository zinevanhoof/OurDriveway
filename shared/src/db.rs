use surrealdb::{
    Surreal,
    engine::remote::ws::{Client, Ws},
    opt::auth::Root,
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
