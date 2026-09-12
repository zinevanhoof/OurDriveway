use diesel::prelude::*;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use shared::error::myerror::MyResult;
use shared::schema::payment::host;
use uuid::Uuid;

/// The `host` table: the slice of a user this service needs to open a Stripe account
/// for them, projected from USERS.
///
/// Two columns and no domain model, for the same reason as
/// [`super::connect_account_repository`]: there is no state to transition and nothing
/// derived. See `migrations/payment/0004_host_mirror/up.sql` for why the values arrive as
/// events rather than as a lookup.
pub struct HostMirrorRepository;

/// What Stripe needs before it will open a connected account.
#[derive(Debug, Queryable, Selectable)]
#[diesel(table_name = shared::schema::payment::host)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct Host {
    pub email: String,
    /// ISO 3166-1 alpha-2, or `None` until the host sets it on their profile.
    pub country: Option<String>,
}

impl HostMirrorRepository {
    pub async fn find(conn: &mut AsyncPgConnection, id: &Uuid) -> MyResult<Option<Host>> {
        Ok(host::table
            .find(id)
            .select(Host::as_select())
            .first(conn)
            .await
            .optional()?)
    }

    /// The row a `Registered` writes.
    ///
    /// **`country` is deliberately absent from the conflict clause.** A `Registered`
    /// carries no country — Stripe fixes it permanently at account creation and the host
    /// sets it on their profile — so a whole-row `.set()` here would clear it every time
    /// USERS replayed. The columns are listed rather than derived for that one reason.
    ///
    /// **`version` is not written here**, and that is what every other projection in this
    /// workspace does: the repository writes the data columns and `shared::set_version!`
    /// writes the version, alone and under `WHERE version < $v`. This used to assign it
    /// inline and unguarded, so a redelivered older event could wind it backwards — the
    /// one table where that was possible. `NOT NULL DEFAULT 0` in
    /// `migrations/payment/0004_host_mirror/up.sql` is what lets the insert omit it.
    pub async fn upsert(conn: &mut AsyncPgConnection, id: &Uuid, email: &str) -> MyResult<()> {
        diesel::insert_into(host::table)
            .values((host::id.eq(id), host::email.eq(email)))
            .on_conflict(host::id)
            .do_update()
            .set(host::email.eq(email))
            .execute(conn)
            .await?;
        Ok(())
    }

    /// What an `Updated` changes.
    ///
    /// `email` and `country` are absent-is-unchanged, the same rule the event itself
    /// carries — `None` there means "not touched", never "clear". Diesel skips a `None`
    /// element of a `set` tuple, which is what `COALESCE($n, column)` used to spell out.
    /// `version` is `set_version!`'s, as in [`Self::upsert`].
    ///
    /// **Does not create the row.** An `Updated` for a user this service has never seen
    /// writes nothing, which is correct rather than lossy: `Registered` precedes it on
    /// the same subject, so it is in the same projector lane and has already been
    /// applied.
    pub async fn patch(
        conn: &mut AsyncPgConnection,
        id: &Uuid,
        email: Option<String>,
        country: Option<String>,
    ) -> MyResult<()> {
        diesel::update(host::table.find(id))
            .set((
                email.map(|e| host::email.eq(e)),
                country.map(|c| host::country.eq(c)),
            ))
            .execute(conn)
            .await?;
        Ok(())
    }
}
