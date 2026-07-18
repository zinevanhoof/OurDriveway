use surrealdb::types::{Datetime, RecordId, SurrealValue, Uuid};

use crate::domain_models::user::User;

#[derive(SurrealValue)]
pub struct RefreshToken {
    pub id: RecordId,
    pub user_id: RecordId,
    pub token_hash: String,
    pub jti: Uuid,
    pub created_at: Datetime,
    pub expires_at: Datetime,
    pub revoked: bool,
    pub revoked_reason: Option<String>,
}

#[derive(SurrealValue)]
pub struct RefreshTokenWithUser {
    pub id: RecordId,
    pub user: User,
    pub token_hash: String,
    pub jti: Uuid,
    pub created_at: Datetime,
    pub expires_at: Datetime,
    pub revoked: bool,
    pub revoked_reason: Option<String>,
}
