use shared::domain_models::user::User;
use shared::error::myerror::{ContextExt, MyResult};
use surrealdb::{Surreal, engine::remote::ws::Client, types::RecordId};

pub struct UserRepository {
    pub db: Surreal<Client>,
}

impl UserRepository {
    pub async fn get_user_by_email_password(
        &self,
        email: &str,
        password: &str,
    ) -> MyResult<Option<User>> {
        let user: Option<User> = self
            .db
            .query(
                r#"
                SELECT id, email, password
                FROM user
                WHERE email = $email
                AND crypto::argon2::compare(password, $password);
            "#,
            )
            .bind(("email", email))
            .bind(("password", password))
            .await?
            .take(0)?;

        Ok(user)
    }

    pub async fn get_user_by_id(&self, id: RecordId) -> MyResult<Option<User>> {
        let user: Option<User> = self
            .db
            .query(
                r#"
                SELECT id, email, password
                FROM user
                WHERE id = $id;
            "#,
            )
            .bind(("id", id))
            .await?
            .take(0)?;

        Ok(user)
    }

    pub async fn create_user(&self, email: &str, password: &str) -> MyResult<User> {
        let user: User = self
            .db
            .query(
                r#"
            CREATE user CONTENT {
                email: $email,
                password: crypto::argon2::generate($password)
            };
            "#,
            )
            .bind(("email", email))
            .bind(("password", password))
            .await?
            .take::<Option<User>>(0)?
            .context_internal("user create returned nothing")?;

        Ok(user)
    }
}
