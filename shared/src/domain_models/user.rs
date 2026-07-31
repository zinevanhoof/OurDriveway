use surrealdb::types::{RecordId, SurrealValue};

#[derive(SurrealValue)]
pub struct User {
    pub id: RecordId,
    pub first_name: String,
    pub last_name: String,
    pub email: String,
    pub password: String,
}
