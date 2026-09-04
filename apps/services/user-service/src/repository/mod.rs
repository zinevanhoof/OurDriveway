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
//! What moved the other way: **writes name their columns**. `UPSERT … CONTENT $row`
//! bound a struct whole, so adding a field to a model needed no edit here. sqlx has no
//! equivalent, so every insert lists its columns and repeats them under `EXCLUDED`.
//! The live tests below are what catch an omission — the compiler will not.
//!
//! The repositories are stateless. They used to be generic over a `Querier` so one
//! type could serve both a service (holding a connection) and a projector (holding an
//! open transaction); sqlx's `PgExecutor` covers both, so each method simply takes
//! one.

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
    use shared::domain_models::user::{RefreshToken, RefreshTokenPatch, User, UserPatch};
    use sqlx::PgPool;
    use uuid::Uuid;

    use super::refresh_token_repository::RefreshTokenRepository;
    use super::user_repository::UserRepository;

    /// Connects **and migrates**, so the only prerequisite is a running container.
    ///
    /// Running the migrations here rather than expecting them applied is what makes
    /// `docker compose up -d yugabyte && cargo test -- --ignored` the whole procedure
    /// — there is no separate schema-import step to forget, the way there was when
    /// the schemas were `.surql` files fed to an endpoint. Idempotent: sqlx records
    /// what it has applied and takes an advisory lock, so concurrent test binaries
    /// are safe.
    async fn db() -> PgPool {
        let pool = shared::db::connect("postgres://yugabyte@127.0.0.1:5433/user")
            .await
            .expect("dev yugabyte on :5433, database `user` — see this module's docs");
        shared::db::migrate(&pool, &sqlx::migrate!("../../../migrations/user"))
            .await
            .expect("migrations apply");
        pool
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

    #[tokio::test]
    #[ignore]
    async fn a_user_round_trips_and_patches_leave_absent_columns_alone() {
        let db = db().await;

        let id = Uuid::now_v7();
        let email = format!("live-{id}@example.test");
        UserRepository::upsert(&db, a_user(id, &email)).await.unwrap();

        let got = UserRepository::find_by_id(&db, id)
            .await
            .unwrap()
            .expect("upserted row");
        assert_eq!(got.id, id);
        assert_eq!(got.version, 1, "the version must survive a whole-row write");
        assert_eq!(got.license_plates, vec!["1-ABC-123".to_string()]);
        assert!(!got.email_verified);

        // `app_user_email_idx … UNIQUE`, which is what makes find_by_email total.
        let by_email = UserRepository::find_by_email(&db, email.clone()).await.unwrap();
        assert_eq!(by_email.expect("same row").id, id);

        UserRepository::patch(
            &db,
            id,
            UserPatch {
                email_verified: Some(true),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        let got = UserRepository::find_by_id(&db, id).await.unwrap().unwrap();
        assert!(got.email_verified);
        // The COALESCE half: everything the patch did not name must survive. This is
        // also what would catch a mis-ordered positional bind, since a swapped pair
        // of `Option<String>`s compiles perfectly well.
        assert_eq!(got.password, "$argon2id$vTEST", "absent columns survive");
        assert_eq!(got.first_name, "Ada");
        assert_eq!(got.last_name, "Lovelace");
        assert_eq!(got.license_plates, vec!["1-ABC-123".to_string()]);

        sqlx::query("DELETE FROM app_user WHERE id = $1")
            .bind(id)
            .execute(&db)
            .await
            .unwrap();
    }

    /// A token round-trips, revoking works, and the sweeper only takes what has
    /// actually expired.
    #[tokio::test]
    #[ignore]
    async fn a_token_round_trips_and_only_expired_ones_are_swept() {
        let db = db().await;

        let user_id = Uuid::now_v7();
        let email = format!("link-{user_id}@example.test");
        UserRepository::upsert(&db, a_user(user_id, &email)).await.unwrap();

        let token_id = Uuid::now_v7();
        let token_hash = format!("hash-{token_id}");
        RefreshTokenRepository::upsert(
            &db,
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

        let got = RefreshTokenRepository::find_by_token_hash(&db, token_hash.clone())
            .await
            .unwrap()
            .expect("issued token");
        assert_eq!(got.id, token_id);
        assert_eq!(got.user_id, user_id);
        assert!(!got.revoked);

        RefreshTokenRepository::patch_by_token_hash(
            &db,
            token_hash.clone(),
            RefreshTokenPatch {
                revoked: Some(true),
                revoked_reason: Some("Rotation".to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        let got = RefreshTokenRepository::find_by_token_hash(&db, token_hash.clone())
            .await
            .unwrap()
            .unwrap();
        assert!(got.revoked);
        assert_eq!(got.revoked_reason.as_deref(), Some("Rotation"));
        assert_eq!(got.user_id, user_id, "a patch must not repoint the owner");

        // Not expired, so the sweep must leave it.
        RefreshTokenRepository::delete_expired(&db, Utc::now()).await.unwrap();
        assert!(
            RefreshTokenRepository::find_by_token_hash(&db, token_hash)
                .await
                .unwrap()
                .is_some(),
            "delete_expired must only take rows already past their expiry"
        );

        sqlx::query("DELETE FROM app_user WHERE id = $1")
            .bind(user_id)
            .execute(&db)
            .await
            .unwrap();
    }

    /// What the schema now promises in place of the hand-written link.
    ///
    /// The old `the_user_link_round_trips` proved two statements agreed with each
    /// other. This proves the database refuses a token that belongs to nobody, which
    /// is the property anyone actually wanted from that column.
    #[tokio::test]
    #[ignore]
    async fn an_orphan_token_is_rejected() {
        let db = db().await;

        let err = RefreshTokenRepository::upsert(
            &db,
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

        // 23503 = foreign_key_violation. Asserted on the code rather than the message
        // so a wording change upstream does not quietly turn this green.
        let shared::error::myerror::MyError::Database(sqlx::Error::Database(e)) = &err else {
            panic!("expected a database error, got {err:?}");
        };
        assert_eq!(e.code().as_deref(), Some("23503"), "{e}");
    }
}
