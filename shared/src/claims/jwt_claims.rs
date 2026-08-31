use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Shared by the issuer (user-service) and every verifier, so the two can't drift.
pub const ISSUER: &str = "OurDriveway";

/// Standard claims, and nothing else.
///
/// Four are gone with the GraphQL proxy: `ns`, `db`, `ac` and an `id` of the form
/// `user:u'<uuid>'`. Those were **SurrealDB's**, not ours — a browser authenticated
/// against the read model directly, and SurrealDB validated a record-access token
/// against the namespace, database and access definition the token itself named. That
/// is why `db` had to say "view" rather than the database user-service writes to, and
/// why `id` needed the `u'…'` literal: it was what made `$auth` resolve to a uuid-keyed
/// record so a permission clause could compare `record::id($auth)` against a column.
///
/// No browser reaches a database now. `sub` is the caller, in the one spelling
/// everything else in the system already uses.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JwtClaims {
    pub iat: i64,
    pub nbf: i64,
    pub exp: i64,
    pub iss: String,
    pub jti: Uuid,
    /// The caller's id. A plain uuid — `AuthedJwt` hands it straight to handlers with
    /// nothing to unwrap, where it used to parse `user:u'<uuid>'` apart first.
    pub sub: Uuid,
}
