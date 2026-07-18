use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize)]
pub struct JwtClaims {
    // ─── Standard JWT claims ─────────────────────
    pub iat: i64,
    pub nbf: i64,
    pub exp: i64,
    pub iss: String,
    pub jti: Uuid,

    // ─── SurrealDB-specific claims ───────────────
    pub ns: String,
    pub db: String,
    pub ac: String,
    pub id: String,
}
