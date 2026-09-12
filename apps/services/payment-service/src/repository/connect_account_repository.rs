use chrono::Utc;
use diesel::prelude::*;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use shared::error::myerror::MyResult;
use shared::schema::payment::connect_account;
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
    pub async fn find(conn: &mut AsyncPgConnection, host_id: &Uuid) -> MyResult<Option<String>> {
        Ok(connect_account::table
            .find(host_id)
            .select(connect_account::stripe_account_id)
            .first(conn)
            .await
            .optional()?)
    }

    /// Records the account, and answers with the one that ends up stored.
    ///
    /// `DO NOTHING` then `SELECT`: two callers arriving together — a double-tapped
    /// "set up payouts" — must not end with two Stripe accounts for one host, and the
    /// PRIMARY KEY is what decides which of them wins. The loser's account is
    /// abandoned at Stripe rather than pointed at, which the idempotency key on
    /// `Stripe::create_account` already makes unlikely: both calls key on the same
    /// host, so Stripe usually hands back the same account to both.
    ///
    /// Returning the stored value rather than the argument is the whole point. A caller
    /// that used its own id after losing this race would talk to an account no row
    /// knows about.
    pub async fn insert(
        conn: &mut AsyncPgConnection,
        host_id: &Uuid,
        stripe_account_id: &str,
    ) -> MyResult<String> {
        // Stays hand-written, and this is the statement least worth fighting the DSL
        // over: a CTE whose INSERT … ON CONFLICT DO NOTHING RETURNING is UNIONed with a
        // plain SELECT so that the loser of the race gets the winner's id back rather
        // than nothing. diesel has no CTE builder, and expressing this as fragments
        // would be the same string with more ceremony.
        let rows: Vec<AccountId> = diesel::sql_query(
            "WITH inserted AS (
                 INSERT INTO connect_account (host_id, stripe_account_id, created_at)
                      VALUES ($1, $2, $3)
                 ON CONFLICT (host_id) DO NOTHING
                   RETURNING stripe_account_id
             )
             SELECT stripe_account_id FROM inserted
              UNION ALL
             SELECT stripe_account_id FROM connect_account WHERE host_id = $1
              LIMIT 1",
        )
        .bind::<diesel::sql_types::Uuid, _>(host_id)
        .bind::<diesel::sql_types::Text, _>(stripe_account_id)
        .bind::<diesel::sql_types::Timestamptz, _>(Utc::now())
        .load(conn)
        .await?;

        rows.into_iter()
            .next()
            .map(|r| r.stripe_account_id)
            .ok_or_else(|| {
                shared::error::myerror::MyError::Bus(
                    "connect_account insert returned no row".to_string(),
                )
            })
    }
}

/// The one column both statements above read back.
#[derive(diesel::QueryableByName)]
struct AccountId {
    #[diesel(sql_type = diesel::sql_types::Text)]
    stripe_account_id: String,
}
