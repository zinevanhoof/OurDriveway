use surrealdb::types::{RecordId, SurrealValue};

#[derive(SurrealValue)]
pub struct User {
    pub id: RecordId,
    pub email: String,
    pub password: String,
}
