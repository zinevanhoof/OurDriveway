use shared::error::myerror::MyResult;
use sqlx::PgExecutor;
use uuid::Uuid;

/// The `host` table: the slice of a user this service needs to open a Stripe account
/// for them, projected from USERS.
///
/// Two columns and no domain model, for the same reason as
/// [`super::connect_account_repository`]: there is no state to transition and nothing
/// derived. See `migrations/payment/0004_host_mirror.sql` for why the values arrive as
/// events rather than as a lookup.
pub struct HostMirrorRepository;

/// What Stripe needs before it will open a connected account.
#[derive(Debug, sqlx::FromRow)]
pub struct Host {
    pub email: String,
    /// ISO 3166-1 alpha-2, or `None` until the host sets it on their profile.
    pub country: Option<String>,
}

impl HostMirrorRepository {
    pub async fn find(ex: impl PgExecutor<'_>, id: &Uuid) -> MyResult<Option<Host>> {
        Ok(
            sqlx::query_as("SELECT email, country FROM host WHERE id = $1")
                .bind(id)
                .fetch_optional(ex)
                .await?,
        )
    }

    /// The row a `Registered` writes.
    pub async fn upsert(
        ex: impl PgExecutor<'_>,
        id: &Uuid,
        version: u64,
        email: &str,
    ) -> MyResult<()> {
        sqlx::query(
            "INSERT INTO host (id, version, email)
                  VALUES ($1, $2, $3)
             ON CONFLICT (id) DO UPDATE SET
                 version = EXCLUDED.version,
                 email   = EXCLUDED.email",
        )
        .bind(id)
        .bind(version as i64)
        .bind(email)
        .execute(ex)
        .await?;
        Ok(())
    }

    /// What an `Updated` changes. `COALESCE` is absent-is-unchanged, the same rule the
    /// event itself carries — `None` there means "not touched", never "clear".
    ///
    /// **Does not create the row.** An `Updated` for a user this service has never seen
    /// writes nothing, which is correct rather than lossy: `Registered` precedes it on
    /// the same subject, so it is in the same projector lane and has already been
    /// applied.
    pub async fn patch(
        ex: impl PgExecutor<'_>,
        id: &Uuid,
        version: u64,
        email: Option<String>,
        country: Option<String>,
    ) -> MyResult<()> {
        sqlx::query(
            "UPDATE host SET
                 version = $2,
                 email   = COALESCE($3, email),
                 country = COALESCE($4, country)
             WHERE id = $1",
        )
        .bind(id)
        .bind(version as i64)
        .bind(email)
        .bind(country)
        .execute(ex)
        .await?;
        Ok(())
    }
}
