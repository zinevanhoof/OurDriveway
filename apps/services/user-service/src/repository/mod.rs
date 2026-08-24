//! One repository per domain, each holding the SurrealQL for the statements this
//! service actually issues. Nothing is generated and no trait sits behind them:
//! what a method does is the string in front of you, with no `format!` and no
//! consts spliced in from the domain models.
//!
//! Reads select `record::id(id) AS id` plus whatever else comes back in a shape the
//! struct cannot deserialize, then `*` — so adding a column to a model needs no
//! edit here.
//!
//! `user` is also written whole with `CONTENT $row`. **`refresh_token` is not**,
//! and it is the only table in the codebase like that: its `user_id` is
//! `record<user>` in the schema and a bare `Uuid` on the struct, so the column has
//! to be re-wrapped on write and unwrapped on read. See
//! [`refresh_token_repository::RefreshTokenRepository::upsert`].
//!
//! Each is generic over its querier so the same type serves both positions — a
//! service holds one over the pooled `Surreal<Client>`, a projector builds one
//! over the open `&Transaction` for a single event.

pub mod refresh_token_repository;
pub mod user_repository;

/// Round-trips both tables through a real SurrealDB.
///
/// `#[ignore]`d — needs `user-service-db` on :8000 with `schemas/user-schema.surql`
/// imported, and CI runs `cargo test --workspace` with no database:
///
/// ```sh
/// docker compose -f docker/docker-compose-dev.yml up -d user-service-db
/// cargo test --workspace -- --ignored
/// ```
///
/// `the_user_link_round_trips` is the one that has to exist. `refresh_token.user_id`
/// is `record<user>` in the schema and a bare `Uuid` on the struct, so the read
/// unwraps and the write re-wraps — two halves that must agree, in two different
/// statements, with nothing in Rust connecting them. Get one without the other and
/// the column round-trips as the wrong type. That used to be a string assertion
/// against the derive's output; the derive is gone, so this is the only place it can
/// be checked at all.
#[cfg(test)]
mod live_tests {
    use std::sync::Arc;

    use chrono::{TimeDelta, Utc};
    use shared::db::Querier;
    use shared::domain_models::user::{RefreshToken, RefreshTokenPatch, User, UserPatch};
    use surrealdb::{Surreal, engine::remote::ws::Client};
    use uuid::Uuid;

    use super::refresh_token_repository::RefreshTokenRepository;
    use super::user_repository::UserRepository;

    async fn db() -> Arc<Surreal<Client>> {
        Arc::new(
            shared::db::connect("127.0.0.1:8000", "root", "root")
                .await
                .expect("user-service-db on :8000 — see this module's docs"),
        )
    }

    async fn drop_row(db: &Arc<Surreal<Client>>, table: &str, id: Uuid) {
        db.q(format!("DELETE type::record('{table}', $v)"))
            .bind(("v", id))
            .await
            .unwrap()
            .check()
            .unwrap();
    }

    fn a_user(id: Uuid, email: &str) -> User {
        User {
            id,
            shard: "00".to_string(),
            first_name: "Ada".to_string(),
            last_name: "Lovelace".to_string(),
            email: email.to_string(),
            password: "$argon2id$vTEST".to_string(),
            profile_picture: None,
            license_plates: vec!["1-ABC-123".to_string()],
            email_verified: false,
        }
    }

    #[tokio::test]
    #[ignore]
    async fn a_user_round_trips_and_patches_leave_absent_columns_alone() {
        let db = db().await;
        let repo = UserRepository { q: db.clone() };

        let id = Uuid::now_v7();
        let email = format!("live-{id}@example.test");
        repo.upsert(a_user(id, &email)).await.unwrap();

        let got = repo.find_by_id(id).await.unwrap().expect("upserted row");
        assert_eq!(
            got.id, id,
            "record::id(id) AS id must unwrap the record key"
        );
        assert_eq!(got.license_plates, vec!["1-ABC-123".to_string()]);
        assert!(!got.email_verified);

        // `email_idx … UNIQUE`, which is what makes the LIMIT 1 a fact.
        let by_email = repo.find_by_email(email.clone()).await.unwrap();
        assert_eq!(by_email.expect("same row").id, id);

        repo.patch(
            id,
            UserPatch {
                email_verified: Some(true),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        let got = repo.find_by_id(id).await.unwrap().unwrap();
        assert!(got.email_verified);
        assert_eq!(got.password, "$argon2id$vTEST", "absent columns survive");
        assert_eq!(got.first_name, "Ada");
        assert_eq!(got.shard, "00", "shard is not patchable and must not move");

        drop_row(&db, "user", id).await;
    }

    /// The read unwraps `record<user>` to a uuid, the write wraps a uuid back into a
    /// record. Both halves, in one round trip, because nothing else connects them.
    #[tokio::test]
    #[ignore]
    async fn the_user_link_round_trips() {
        let db = db().await;
        let users = UserRepository { q: db.clone() };
        let tokens = RefreshTokenRepository { q: db.clone() };

        let user_id = Uuid::now_v7();
        let email = format!("link-{user_id}@example.test");
        users.upsert(a_user(user_id, &email)).await.unwrap();

        let token_id = Uuid::now_v7();
        let token_hash = format!("hash-{token_id}");
        tokens
            .upsert(RefreshToken {
                id: token_id,
                user_id,
                shard: "00".to_string(),
                token_hash: token_hash.clone(),
                jti: Uuid::now_v7().into(),
                created_at: Utc::now().into(),
                expires_at: (Utc::now() + TimeDelta::days(30)).into(),
                revoked: false,
                revoked_reason: None,
            })
            .await
            .unwrap();

        let got = tokens
            .find_by_token_hash(token_hash.clone())
            .await
            .unwrap()
            .expect("issued token");
        assert_eq!(got.id, token_id);
        // The whole point: this came back a plain uuid, from a column the schema
        // declares `record<user>`.
        assert_eq!(got.user_id, user_id);
        assert!(!got.revoked);

        // And it really is a record on disk, not a uuid the write happened to store.
        let linked: Option<Uuid> = db
            .q("SELECT VALUE record::id(user_id) FROM ONLY type::record('refresh_token', $v)")
            .bind(("v", token_id))
            .await
            .unwrap()
            .take(0)
            .unwrap();
        assert_eq!(
            linked,
            Some(user_id),
            "user_id must be stored as user:⟨uuid⟩, not a bare uuid"
        );

        // Revoking must not disturb the link.
        tokens
            .patch_by_token_hash(
                token_hash.clone(),
                RefreshTokenPatch {
                    revoked: Some(true),
                    revoked_reason: Some("Rotation".to_string()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let got = tokens
            .find_by_token_hash(token_hash)
            .await
            .unwrap()
            .unwrap();
        assert!(got.revoked);
        assert_eq!(got.revoked_reason.as_deref(), Some("Rotation"));
        assert_eq!(got.user_id, user_id, "a patch must not repoint the link");

        // Expired rows are swept; this one is not expired, so it must survive.
        tokens.delete_expired(Utc::now()).await.unwrap();
        assert!(
            tokens
                .find_by_token_hash(format!("hash-{token_id}"))
                .await
                .unwrap()
                .is_some(),
            "delete_expired must only take rows already past their expiry"
        );

        drop_row(&db, "refresh_token", token_id).await;
        drop_row(&db, "user", user_id).await;
    }
}
