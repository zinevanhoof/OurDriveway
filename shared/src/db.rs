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
pub async fn connect(addr: &str) -> MyResult<Surreal<Client>> {
    let db = Surreal::new::<Ws>(addr).await?;

    db.signin(Root {
        username: std::env::var("SURREALDB_USER").unwrap_or_else(|_| "root".into()),
        password: std::env::var("SURREALDB_PASS").unwrap_or_else(|_| "root".into()),
    })
    .await?;

    db.use_ns("main").use_db("main").await?;

    Ok(db)
}
