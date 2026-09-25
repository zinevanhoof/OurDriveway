use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum UserEvent {
    Registered(UserRegistered),
    Updated(UserUpdated),
    PasswordChanged(UserPasswordChanged),
    /// The address was proven reachable — somebody opened a link only that
    /// mailbox received. Published by user-service after checking the token.
    EmailVerified {
        user_id: Uuid,
    },
    /// "Send that link again." Raised by user-service when a user asks for a new
    /// verification email, and consumed only by notification-service — nothing
    /// projects it.
    ///
    /// Carries the address and name rather than just an id so notification-service
    /// never has to look a user up, which is what lets it own no database at all.
    VerificationRequested(VerificationRequested),
    /// "I've forgotten it." Same shape and same reason as
    /// [`Self::VerificationRequested`], and likewise projected by nothing.
    ///
    /// **The envelope's `version` is load-bearing for this one.**
    /// notification-service mints the reset token carrying it, and user-service
    /// refuses that token unless the row is still at that version — which is the
    /// whole of what makes a reset link single-use with nothing stored anywhere.
    /// A consumer that re-raises this event under a different version silently
    /// breaks that.
    PasswordResetRequested(PasswordResetRequested),
    /// The user opened their notifications. The envelope's `occurred_at` is the
    /// watermark: everything visible before it now reads as seen.
    ///
    /// Projected by view-service only. Not stored by user-service, so a backfill
    /// cannot reproduce it — a rebuilt view shows every open notification as new
    /// once more, which costs a badge and nothing else.
    NotificationsSeen {
        user_id: Uuid,
    },
    /// The user dismissed one notification — the way a kind with no event of its own to
    /// finish it (`spot_booked`) is handled. Projected by view-service only, and scoped
    /// to `user_id` there, so nobody can dismiss someone else's.
    NotificationDismissed {
        user_id: Uuid,
        kind: String,
        subject_id: Uuid,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VerificationRequested {
    pub user_id: Uuid,
    pub email: String,
    pub first_name: String,
}

/// Deliberately its own struct rather than a second variant over
/// [`VerificationRequested`]: the two are the same three fields today, and the
/// name is what a reader of notification-service's `mail_for` match sees.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PasswordResetRequested {
    pub user_id: Uuid,
    pub email: String,
    pub first_name: String,
}

impl UserEvent {
    /// The user every variant is about — the aggregate half of `user:<uuid>`.
    ///
    /// Saves each consumer re-deriving it with its own match, and makes a new
    /// variant that forgets to carry a user id a compile error here rather than a
    /// silently unversioned row somewhere downstream.
    pub fn user_id(&self) -> Uuid {
        match self {
            Self::Registered(e) => e.user_id,
            Self::Updated(e) => e.user_id,
            Self::PasswordChanged(e) => e.user_id,
            Self::EmailVerified { user_id }
            | Self::NotificationsSeen { user_id }
            | Self::NotificationDismissed { user_id, .. } => *user_id,
            Self::VerificationRequested(e) => e.user_id,
            Self::PasswordResetRequested(e) => e.user_id,
        }
    }
}

/// **No password, in any form.** It used to carry the Argon2 hash, and nothing
/// downstream ever read it: view-service's row has no such column, payment-service
/// mirrors an id and an address, and notification-service wants a name. The only
/// reads were user-service building its own row out of a struct it was about to
/// publish — a local convenience, never a contract — so the hash was travelling
/// through a stream four services consume purely because of where the writer
/// happened to keep it. `User::registered` takes it as an argument now.
///
/// The event-sourcing case for carrying it is gone too: STREAM_USERS expires after
/// a week (see [`crate::events::STREAMS`]), so nothing older is rebuildable from
/// the log, and user-service's row is read rather than re-derived — see
/// `UserService::backfill`, which no longer re-emits every account's hash.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UserRegistered {
    pub user_id: Uuid,
    pub first_name: String,
    pub last_name: String,
    pub email: String,
}

/// Partial update — `None` means "unchanged", not "clear".
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UserUpdated {
    pub user_id: Uuid,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub profile_picture: Option<String>,
    pub email: Option<String>,
    /// The whole list, or `None`. There is no add/remove event: the edit form
    /// submits every plate it knows about, so a diff would mean the client
    /// deciding what "unchanged" is.
    pub license_plates: Option<Vec<String>>,
    /// ISO 3166-1 alpha-2, uppercase, or `None` for unchanged.
    ///
    /// The one field on this event that another *service* needs rather than a
    /// screen: payment-service mirrors it, because Stripe will not open a connected
    /// account without a country and fixes it permanently at creation.
    pub country: Option<String>,
}

/// A marker: this account's password is not what it was. Carries no password, for
/// the reasons on [`UserRegistered`].
///
/// Still its own event rather than a field on [`UserUpdated`], and now for the only
/// reason that was ever load-bearing: a consumer that wants to react to a password
/// changing — revoke sessions, mail the account, raise an alert — has to be able to
/// match on it rather than sniff a field. What it must *not* do is learn the new
/// password, which was true before and is now true by construction.
///
/// Deliberately the same bare shape as [`UserEvent::EmailVerified`]. Both say only
/// that something happened to an account the reader can look up if it is entitled to.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UserPasswordChanged {
    pub user_id: Uuid,
}
