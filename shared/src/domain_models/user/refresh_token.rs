use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::events::session::{RefreshTokenIssued, RefreshTokenRotated};

/// The `refresh_token` table.
///
/// Only ever the **hash** of a token, never the token itself — the plaintext is a
/// bearer credential and goes to the client's cookie and nowhere else.
#[derive(Clone, Debug, sqlx::FromRow)]
pub struct RefreshToken {
    pub id: Uuid,
    /// A plain uuid with a foreign key to `app_user(id)`.
    ///
    /// This was the codebase's **only linked column** — `record<user>` in the schema
    /// but a bare uuid on the struct, so reads unwrapped it with
    /// `record::id(user_id)` and writes re-wrapped it with
    /// `type::record('user', $user_id)`. Two halves in two different statements with
    /// nothing in Rust connecting them, which is why it was also the one table whose
    /// upsert could not be `CONTENT $row`, and why a live test existed purely to
    /// prove the two halves agreed.
    ///
    /// All of that is gone. The column is a uuid, the write binds a uuid, and the
    /// foreign key is what enforces the relationship — so the live test can assert
    /// something real (an orphan is rejected) instead of asserting that two hand-
    /// written statements still match each other.
    pub user_id: Uuid,
    /// `refresh_token_hash_idx … UNIQUE`. The only way a token is ever looked up:
    /// refresh and logout both arrive holding a plaintext token and nothing else.
    pub token_hash: String,
    pub jti: Uuid,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
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

// No `bind` here any more — see the note in the sibling `user.rs`. sqlx binds
// positionally, so the binds live beside the `$n` placeholders in
// `RefreshTokenRepository::patch_by_token_hash`.

impl RefreshToken {
    /// The row an `Issued` writes. `at` is the envelope's clock, not this
    /// process's — every replica must store the same `created_at`.
    pub fn issued(e: RefreshTokenIssued, at: DateTime<Utc>) -> Self {
        Self {
            id: e.token_id,
            user_id: e.user_id,
            token_hash: e.token_hash,
            jti: e.jti,
            created_at: at,
            expires_at: e.expires_at,
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
            token_hash: e.token_hash,
            jti: e.jti,
            created_at: at,
            expires_at: e.expires_at,
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
    /// reminder that the `SET` list in `patch_by_token_hash` needs it too, and a
    /// `.bind()` in the matching position.
    ///
    /// `user_id` is not a field here, so a partial update cannot repoint the foreign
    /// key. That used to matter twice over, because the link had a read/write
    /// asymmetry to get right in every statement touching it; the column is a plain
    /// uuid with a real FK now, so this is only about not repointing it.
    #[test]
    fn set_covers_every_patchable_column() {
        let _: RefreshTokenPatch = RefreshTokenPatch {
            revoked: None,
            revoked_reason: None,
        };
    }
}
