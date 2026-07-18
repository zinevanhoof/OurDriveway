use chrono::{Duration, Utc};
use shared::domain_models::user::User;
use shared::error::myerror::{ContextExt, MyError, MyResult};
use uuid::Uuid;

use crate::{
    CONFIG,
    auth::jwt::generate_jwt,
    repository::{
        refresh_token_repository::RefreshTokenRepository, user_repository::UserRepository,
    },
};

pub struct UserService {
    pub user_repository: UserRepository,
    pub refresh_token_repository: RefreshTokenRepository,
}

impl UserService {
    pub async fn signup(&self, email: &str, password: &str) -> MyResult<User> {
        Ok(self.user_repository.create_user(email, password).await?)
    }

    pub async fn login(&self, email: &str, password: &str) -> MyResult<(String, String)> {
        let user: User = self
            .user_repository
            .get_user_by_email_password(email, password)
            .await?
            .context_unauthorized(("Unauthorized", "Invalid credentials"))?;

        let now = Utc::now();
        let jwt_exp = Utc::now() + Duration::minutes(CONFIG.jwt_expiration);
        let refresh_token_exp = Utc::now() + Duration::days(CONFIG.refresh_token_expiration);
        let jti = Uuid::new_v4();

        let jwt = generate_jwt(&user.id, jwt_exp, jti)?;

        let refresh_token = Uuid::new_v4();

        self.refresh_token_repository
            .create_refresh_token(user.id, refresh_token, jti, now, refresh_token_exp)
            .await?;

        Ok((jwt, refresh_token.to_string()))
    }

    pub async fn logout(&self, refresh_token: Uuid) -> MyResult<()> {
        self.refresh_token_repository
            .revoke_refresh_token(refresh_token, "logout")
            .await?;

        Ok(())
    }

    pub async fn refresh(&self, old_refresh_token: Uuid) -> MyResult<(String, String)> {
        let refresh_token = self
            .refresh_token_repository
            .get_refresh_token(old_refresh_token)
            .await?
            .context_not_found(("Not Found", "Could not find refresh token"))?;

        let now = Utc::now();
        let jwt_exp = Utc::now() + Duration::minutes(CONFIG.jwt_expiration);
        let refresh_token_exp = Utc::now() + Duration::days(CONFIG.refresh_token_expiration);
        let jti = Uuid::new_v4();

        if refresh_token.revoked || refresh_token.expires_at.into_inner() < now {
            Err(MyError::unauthorized("Unauthorized", "Invalid refresh token"))?
        }

        let jwt = generate_jwt(&refresh_token.user_id, jwt_exp, jti)?;

        let refresh_token = Uuid::new_v4();

        self.refresh_token_repository
            .renew_refresh_token(
                old_refresh_token,
                refresh_token,
                jti,
                now,
                refresh_token_exp,
            )
            .await?;

        Ok((jwt, refresh_token.to_string()))
    }
}
