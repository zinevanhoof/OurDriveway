//! One repository per domain, each holding the SQL for the statements this service
//! actually issues. Nothing is generated and no trait sits behind them: what a method
//! does is the string in front of you, with no `format!` and no consts spliced in
//! from the domain models. Runtime-checked `query_as`, never `query_as!` — so no
//! statement here needs a live database at compile time.
//!
//! Reads are plain `SELECT *`. Every one of them used to carry `record::id(id) AS id`
//! plus a list of `?? default` fallbacks, because the id was a record key rather than
//! a column and older rows held NONE where a field had been added since. Both are
//! gone: the id is a uuid column, and the schema's `NOT NULL DEFAULT` leaves no absent
//! case.
//!
//! **Writes bind the struct whole again.** `UPSERT … CONTENT $row` did that under
//! SurrealDB; sqlx had no equivalent, so for a while every insert listed its columns
//! and repeated them under `EXCLUDED`, with only the live tests below to catch an
//! omission. `#[derive(Insertable, AsChangeset)]` gives it back, and this time the
//! compiler checks the columns against `shared::schema`.
//!
//! The repositories are stateless. They used to be generic over a `Querier` so one
//! type could serve both a service and a projector; every method now takes
//! `&mut AsyncPgConnection`, which is what a pooled connection and an open transaction
//! both are.

pub mod refresh_token_repository;
pub mod user_repository;

/// Round-trips both tables through a real YugabyteDB.
///
/// `#[ignore]`d — needs the dev cluster on :5433 with `migrations/user` applied, and
/// CI runs `cargo test --workspace` with no database:
///
/// ```sh
/// docker compose -f docker/docker-compose-dev.yml up -d yugabyte
/// cargo test --workspace -- --ignored
/// ```
///
/// `the_user_link_round_trips` is gone, and what it was protecting is worth recording
/// because its disappearance is the point. `refresh_token.user_id` was `record<user>`
/// in the schema and a bare `Uuid` on the struct, so the read unwrapped and the write
/// re-wrapped — two halves, in two different statements, with nothing in Rust
/// connecting them. Get one without the other and the column round-tripped as the
/// wrong type. That test existed because no compiler could see the coupling.
///
/// The column is a uuid with a foreign key now. There is no asymmetry to get wrong, so
/// the test asserts something the schema actually promises instead:
/// `an_orphan_token_is_rejected`.
#[cfg(test)]
mod live_tests {
    use chrono::{TimeDelta, Utc};
    use diesel::prelude::*;
    use diesel_async::RunQueryDsl;
    use shared::domain_models::user::{RefreshToken, RefreshTokenPatch, User, UserPatch};
    use shared::schema::user::app_user;
    use uuid::Uuid;

    use super::refresh_token_repository::RefreshTokenRepository;
    use super::user_repository::UserRepository;

    /// Connects **and migrates**, so the only prerequisite is a running container.
    ///
    /// Running the migrations here rather than expecting them applied is what makes
    /// `docker compose up -d yugabyte && cargo test -- --ignored` the whole procedure
    /// — there is no separate schema-import step to forget.
    ///
    /// Through `migrator::run_one` rather than a second copy of the wiring: that crate
    /// is the only thing that migrates in dev and in production too, so a test cannot
    /// drift from what actually gets applied. It creates the database if it is missing
    /// and is a no-op once applied, so concurrent test binaries are safe.
    async fn db() -> shared::db::Db {
        // SAFETY of the `set_var`: tests in one binary share an environment, and every
        // caller here sets the same value.
        unsafe {
            std::env::set_var(
                "USER_DATABASE_URL",
                "postgres://yugabyte@127.0.0.1:5433/user",
            )
        };
        migrator::ensure("user").await.expect("migrations apply");

        shared::db::connect("postgres://yugabyte@127.0.0.1:5433/user")
            .await
            .expect("dev yugabyte on :5433, database `user` — see this module's docs")
    }

    /// One connection for a test to pass around, since repositories take a connection
    /// rather than a pool.
    async fn conn(
        db: &shared::db::Db,
    ) -> diesel_async::pooled_connection::bb8::PooledConnection<'_, diesel_async::AsyncPgConnection>
    {
        shared::db::conn(db).await.expect("a connection")
    }

    fn a_user(id: Uuid, email: &str) -> User {
        User {
            id,
            version: 1,
            first_name: "Ada".to_string(),
            last_name: "Lovelace".to_string(),
            email: email.to_string(),
            password: "$argon2id$vTEST".to_string(),
            profile_picture: None,
            license_plates: vec!["1-ABC-123".to_string()],
            email_verified: false,
            country: None,
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn a_user_round_trips_and_patches_leave_absent_columns_alone() {
        let pool = db().await;
        let db = &mut *conn(&pool).await;

        let id = Uuid::now_v7();
        let email = format!("live-{id}@example.test");
        UserRepository::upsert(db, a_user(id, &email))
            .await
            .unwrap();

        let got = UserRepository::find_by_id(db, id)
            .await
            .unwrap()
            .expect("upserted row");
        assert_eq!(got.id, id);
        assert_eq!(got.version, 1, "the version must survive a whole-row write");
        assert_eq!(got.license_plates, vec!["1-ABC-123".to_string()]);
        assert!(!got.email_verified);

        // `app_user_email_idx … UNIQUE`, which is what makes find_by_email total.
        let by_email = UserRepository::find_by_email(db, email.clone())
            .await
            .unwrap();
        assert_eq!(by_email.expect("same row").id, id);

        UserRepository::patch(
            db,
            id,
            UserPatch {
                email_verified: Some(true),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        let got = UserRepository::find_by_id(db, id).await.unwrap().unwrap();
        assert!(got.email_verified);
        // The COALESCE half: everything the patch did not name must survive. This is
        // also what would catch a mis-ordered positional bind, since a swapped pair
        // of `Option<String>`s compiles perfectly well.
        assert_eq!(got.password, "$argon2id$vTEST", "absent columns survive");
        assert_eq!(got.first_name, "Ada");
        assert_eq!(got.last_name, "Lovelace");
        assert_eq!(got.license_plates, vec!["1-ABC-123".to_string()]);

        diesel::delete(app_user::table.find(id))
            .execute(&mut *conn(&pool).await)
            .await
            .unwrap();
    }

    /// A token round-trips, revoking works, and the sweeper only takes what has
    /// actually expired.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn a_token_round_trips_and_only_expired_ones_are_swept() {
        let pool = db().await;
        let db = &mut *conn(&pool).await;

        let user_id = Uuid::now_v7();
        let email = format!("link-{user_id}@example.test");
        UserRepository::upsert(db, a_user(user_id, &email))
            .await
            .unwrap();

        let token_id = Uuid::now_v7();
        let token_hash = format!("hash-{token_id}");
        RefreshTokenRepository::upsert(
            db,
            RefreshToken {
                id: token_id,
                user_id,
                token_hash: token_hash.clone(),
                jti: Uuid::now_v7(),
                created_at: Utc::now(),
                expires_at: Utc::now() + TimeDelta::days(30),
                revoked: false,
                revoked_reason: None,
            },
        )
        .await
        .unwrap();

        let got = RefreshTokenRepository::find_by_token_hash(db, token_hash.clone())
            .await
            .unwrap()
            .expect("issued token");
        assert_eq!(got.id, token_id);
        assert_eq!(got.user_id, user_id);
        assert!(!got.revoked);

        RefreshTokenRepository::patch_by_token_hash(
            db,
            token_hash.clone(),
            RefreshTokenPatch {
                revoked: Some(true),
                revoked_reason: Some("Rotation".to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        let got = RefreshTokenRepository::find_by_token_hash(db, token_hash.clone())
            .await
            .unwrap()
            .unwrap();
        assert!(got.revoked);
        assert_eq!(got.revoked_reason.as_deref(), Some("Rotation"));
        assert_eq!(got.user_id, user_id, "a patch must not repoint the owner");

        // Not expired, so the sweep must leave it.
        RefreshTokenRepository::delete_expired(db, Utc::now())
            .await
            .unwrap();
        assert!(
            RefreshTokenRepository::find_by_token_hash(db, token_hash)
                .await
                .unwrap()
                .is_some(),
            "delete_expired must only take rows already past their expiry"
        );

        diesel::delete(app_user::table.find(user_id))
            .execute(&mut *conn(&pool).await)
            .await
            .unwrap();
    }

    /// What the schema now promises in place of the hand-written link.
    ///
    /// The old `the_user_link_round_trips` proved two statements agreed with each
    /// other. This proves the database refuses a token that belongs to nobody, which
    /// is the property anyone actually wanted from that column.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn an_orphan_token_is_rejected() {
        let pool = db().await;
        let db = &mut *conn(&pool).await;

        let err = RefreshTokenRepository::upsert(
            db,
            RefreshToken {
                id: Uuid::now_v7(),
                // Nobody.
                user_id: Uuid::now_v7(),
                token_hash: format!("orphan-{}", Uuid::now_v7()),
                jti: Uuid::now_v7(),
                created_at: Utc::now(),
                expires_at: Utc::now() + TimeDelta::days(30),
                revoked: false,
                revoked_reason: None,
            },
        )
        .await
        .expect_err("a token for a user that does not exist must be refused");

        // Asserted on the KIND rather than on the message, so a wording change upstream
        // does not quietly turn this green.
        //
        // This got stronger in the move to diesel. It used to compare SQLSTATE `23503`
        // as a string; diesel-async maps that code to a typed variant, so the assertion
        // is now a pattern the compiler checks rather than a literal that could go stale.
        assert!(
            matches!(
                &err,
                shared::error::myerror::MyError::Database(diesel::result::Error::DatabaseError(
                    diesel::result::DatabaseErrorKind::ForeignKeyViolation,
                    _
                ))
            ),
            "expected a foreign key violation, got {err:?}"
        );
    }
}
