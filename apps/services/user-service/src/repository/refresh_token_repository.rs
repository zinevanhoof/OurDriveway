use chrono::{DateTime, Utc};
use shared::error::myerror::{ContextExt, MyResult};
use shared::domain_models::refresh_token::{RefreshToken, RefreshTokenWithUser};
use surrealdb::{Surreal, engine::remote::ws::Client, types::RecordId};
use uuid::Uuid;

pub struct RefreshTokenRepository {
    pub db: Surreal<Client>,
}

impl RefreshTokenRepository {
    pub async fn get_refresh_token(&self, refresh_token: Uuid) -> MyResult<Option<RefreshToken>> {
        let refresh_token: Option<RefreshToken> = self
            .db
            .query(
                "
                SELECT *
                FROM refresh_token
                WHERE token_hash = crypto::sha256($refresh_token);
            ",
            )
            .bind(("refresh_token", refresh_token.to_string()))
            .await?
            .take(0)?;

        Ok(refresh_token)
    }

    pub async fn get_refresh_token_with_user(
        &self,
        refresh_token: Uuid,
    ) -> MyResult<Option<RefreshTokenWithUser>> {
        let refresh_token: Option<RefreshTokenWithUser> = self
            .db
            .query(
                "
                SELECT *, user_id AS user
                FROM refresh_token
                WHERE token_hash = crypto::sha256($refresh_token)
                FETCH user;
            ",
            )
            .bind(("refresh_token", refresh_token.to_string()))
            .await?
            .take(0)?;

        Ok(refresh_token)
    }

    pub async fn create_refresh_token(
        &self,
        user_id: RecordId,
        refresh_token: Uuid,
        jti: Uuid,
        created_at: DateTime<Utc>,
        expires_at: DateTime<Utc>,
    ) -> MyResult<RefreshToken> {
        let refresh_token: RefreshToken = self
            .db
            .query(
                r#"
            CREATE refresh_token SET
                user_id = $user_id,
                token_hash = crypto::sha256($refresh_token),
                jti = $jti,
                created_at = $created_at,
                expires_at = $expires_at;
            "#,
            )
            .bind(("user_id", user_id))
            .bind(("refresh_token", refresh_token.to_string()))
            .bind(("jti", jti))
            .bind(("created_at", created_at))
            .bind(("expires_at", expires_at))
            .await?
            .take::<Option<RefreshToken>>(0)?
            .context_internal("refresh_token query returned nothing")?;

        Ok(refresh_token)
    }

    pub async fn revoke_refresh_token(&self, refresh_token: Uuid, reason: &str) -> MyResult<()> {
        self.db
            .query(
                r#"
            UPDATE refresh_token SET
                revoked = true,
                revoked_reason = $reason
                WHERE token_hash = crypto::sha256($refresh_token);
            "#,
            )
            .bind(("reason", reason))
            .bind(("refresh_token", refresh_token.to_string()))
            .await?;

        Ok(())
    }

    pub async fn renew_refresh_token(
        &self,
        old_refresh_token: Uuid,
        new_refresh_token: Uuid,
        jti: Uuid,
        created_at: DateTime<Utc>,
        expires_at: DateTime<Utc>,
    ) -> MyResult<RefreshToken> {
        let refresh_token: RefreshToken = self
            .db
            .query(
                "
                LET $user_id = (
                UPDATE ONLY refresh_token SET
                    revoked = true,
                    revoked_reason = $reason
                WHERE token_hash = crypto::sha256($old_refresh_token)
                RETURN VALUE user_id
                );

                CREATE refresh_token SET
                user_id = $user_id,
                token_hash = crypto::sha256($new_refresh_token),
                jti = $jti,
                created_at = $created_at,
                expires_at = $expires_at;
            ",
            )
            .bind(("reason", "Rotation"))
            .bind(("old_refresh_token", old_refresh_token.to_string()))
            .bind(("new_refresh_token", new_refresh_token.to_string()))
            .bind(("jti", jti))
            .bind(("created_at", created_at))
            .bind(("expires_at", expires_at))
            .await?
            .take::<Option<RefreshToken>>(1)?
            .context_internal("refresh_token query returned nothing")?;

        Ok(refresh_token)
    }
}
