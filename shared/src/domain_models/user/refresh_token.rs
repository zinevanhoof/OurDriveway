use chrono::{DateTime, Utc};
use surrealdb::types::{Datetime, SurrealValue};
use uuid::Uuid;

use surrealdb::{engine::remote::ws::Client, method::Query};

use crate::events::session::{RefreshTokenIssued, RefreshTokenRotated};

/// The `refresh_token` table.
///
/// Only ever the **hash** of a token, never the token itself — the plaintext is a
/// bearer credential and goes to the client's cookie and nowhere else.
#[derive(Clone, Debug, SurrealValue)]
pub struct RefreshToken {
    pub id: Uuid,
    /// Stored as a link to `user:⟨uuid⟩` — `user_id ON refresh_token TYPE
    /// record<user>` — but held here as the bare uuid. Reads unwrap it with
    /// `record::id(user_id) AS user_id`, writes re-wrap it with
    /// `type::record('user', $user_id)`.
    ///
    /// **The only linked column in the codebase**, and the reason this is the one
    /// table whose upsert cannot be `CONTENT $row`: binding the struct whole would
    /// send a plain uuid where the schema demands a record. Everywhere else a
    /// cross-service id is a plain uuid, because the table it points at lives in
    /// another database. Here both tables are in this one.
    pub user_id: Uuid,
    /// The *user's* shard — sessions live on the user's subject so one user's
    /// whole session history stays on one ordered subject.
    pub shard: String,
    /// `refresh_token_hash … UNIQUE`. The only way a token is ever looked up:
    /// refresh and logout both arrive holding a plaintext token and nothing else.
    ///
    /// The index carried no `UNIQUE` until this was marked — the lookup had always
    /// assumed one and said `LIMIT 1`, but nothing enforced it.
    pub token_hash: String,
    pub jti: surrealdb::types::Uuid,
    pub created_at: Datetime,
    pub expires_at: Datetime,
    pub revoked: bool,
    pub revoked_reason: Option<String>,
}

/// A partial update to a [`RefreshToken`], written with the struct-update idiom:
///
/// ```ignore
/// RefreshTokenPatch { revoked: Some(true), ..Default::default() }
/// ```
///
/// Two columns, because revoking is the only thing that ever happens to a token
/// after it is issued. Everything else — the hash, the jti, the expiry, and the
/// user it belongs to — is fixed at issue and not representable here.
#[derive(Debug, Default)]
pub struct RefreshTokenPatch {
    pub revoked: Option<bool>,
    pub revoked_reason: Option<String>,
}

impl RefreshTokenPatch {
    /// Binds every patchable column. Absent ones bind as NONE, which the
    /// `?? column` in `RefreshTokenRepository::patch_by_token_hash` turns into
    /// "leave it alone".
    pub fn bind(self, q: Query<'_, Client>) -> Query<'_, Client> {
        q.bind(("revoked", self.revoked))
            .bind(("revoked_reason", self.revoked_reason))
    }
}

impl RefreshToken {
    /// The row an `Issued` writes. `at` is the envelope's clock, not this
    /// process's — every replica must store the same `created_at`.
    pub fn issued(e: RefreshTokenIssued, at: DateTime<Utc>) -> Self {
        Self {
            id: e.token_id,
            user_id: e.user_id,
            shard: e.shard,
            token_hash: e.token_hash,
            jti: e.jti.into(),
            created_at: at.into(),
            expires_at: e.expires_at.into(),
            revoked: false,
            revoked_reason: None,
        }
    }

    /// The issue half of a `Rotated`. The revoke half is a patch on the old hash;
    /// both run in the projector's one transaction, which is what makes rotation
    /// atomic in the projection as well as in the log.
    pub fn rotated(e: RefreshTokenRotated, at: DateTime<Utc>) -> Self {
        Self {
            id: e.token_id,
            user_id: e.user_id,
            shard: e.shard,
            token_hash: e.token_hash,
            jti: e.jti.into(),
            created_at: at.into(),
            expires_at: e.expires_at.into(),
            revoked: false,
            revoked_reason: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The SQL lives in `RefreshTokenRepository` now, so nothing here can assert on
    /// it. What this *can* do is fail the moment the struct grows a field.
    ///
    /// The literal is **exhaustive on purpose** — no `..Default::default()`. Add a
    /// field to [`RefreshTokenPatch`] and this stops compiling, which is the
    /// reminder that `bind` and the `SET` list in `patch_by_token_hash` need it too.
    ///
    /// `user_id` is not a field here, so a partial update cannot repoint the
    /// foreign key — which also means the link's read/write asymmetry only has to
    /// be got right in two statements, not three. `the_user_link_round_trips` in
    /// user-service checks those against a real database.
    #[test]
    fn set_covers_every_patchable_column() {
        let _: RefreshTokenPatch = RefreshTokenPatch {
            revoked: None,
            revoked_reason: None,
        };
    }
}
