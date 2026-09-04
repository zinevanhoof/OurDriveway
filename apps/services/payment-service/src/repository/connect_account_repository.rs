use chrono::Utc;
use shared::error::myerror::MyResult;
use sqlx::PgExecutor;
use uuid::Uuid;

/// The `connect_account` table: which Stripe account a host is paid into.
///
/// Two statements, and no domain model behind them. The row is a host id and a Stripe
/// id — there is no state to transition, nothing derived, and nothing else this service
/// stores about the account. Whether that account can actually *be* paid is Stripe's
/// answer, fetched live by `ConnectService::status`, never a column here.
pub struct ConnectAccountRepository;

impl ConnectAccountRepository {
    /// The host's `acct_…`, or `None` if they have never started onboarding.
    pub async fn find(ex: impl PgExecutor<'_>, owner_id: &Uuid) -> MyResult<Option<String>> {
        Ok(
            sqlx::query_scalar("SELECT stripe_account_id FROM connect_account WHERE owner_id = $1")
                .bind(owner_id)
                .fetch_optional(ex)
                .await?,
        )
    }

    /// Records the account, and answers with the one that ends up stored.
    ///
    /// `DO NOTHING` then `SELECT`: two callers arriving together — a double-tapped
    /// "set up payouts" — must not end with two Stripe accounts for one host, and the
    /// PRIMARY KEY is what decides which of them wins. The loser's account is
    /// abandoned at Stripe rather than pointed at, which the idempotency key on
    /// `Stripe::create_account` already makes unlikely: both calls key on the same
    /// owner, so Stripe usually hands back the same account to both.
    ///
    /// Returning the stored value rather than the argument is the whole point. A caller
    /// that used its own id after losing this race would talk to an account no row
    /// knows about.
    pub async fn insert(
        ex: impl PgExecutor<'_>,
        owner_id: &Uuid,
        stripe_account_id: &str,
    ) -> MyResult<String> {
        Ok(sqlx::query_scalar(
            "WITH inserted AS (
                 INSERT INTO connect_account (owner_id, stripe_account_id, created_at)
                      VALUES ($1, $2, $3)
                 ON CONFLICT (owner_id) DO NOTHING
                   RETURNING stripe_account_id
             )
             SELECT stripe_account_id FROM inserted
              UNION ALL
             SELECT stripe_account_id FROM connect_account WHERE owner_id = $1
              LIMIT 1",
        )
        .bind(owner_id)
        .bind(stripe_account_id)
        .bind(Utc::now())
        .fetch_one(ex)
        .await?)
    }
}
