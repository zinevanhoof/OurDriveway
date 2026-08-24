use async_nats::jetstream::Context;
use shared::domain_models::user::User;
use shared::error::myerror::{ContextExt, MyResult};
use shared::events::user::{
    UserEvent, UserPasswordChanged, UserRegistered, UserUpdated, VerificationRequested,
};
use shared::events::{Envelope, shard_of, user_subject};
use shared::requests::user::{
    Email, LoginRequest, ResendVerificationRequest, SignupRequest, UpdateUserRequest,
    VerifyEmailRequest,
};
use uuid::Uuid;

use crate::{CONFIG, auth::password, repository::user_repository::UserRepository};

/// Write side for the user itself: who they are, and proving it.
///
/// Nothing here writes to the database — the projectors do, from the same events
/// every other instance consumes. Reads go straight to the models in
/// `shared::domain_models`.
///
/// Sessions are [`crate::service::refresh_token_service::RefreshTokenService`]'s.
/// This one stops at `authenticate`, which answers "are these credentials good"
/// and hands back the row; deciding what token to mint for it is the other
/// service's call.
/// No `await_applied` anywhere below, deliberately.
///
/// Every write here answers with the position its event landed at, and the client
/// echoes it as `X-Await-Seq`; the [`bus::await_seq`] layer mounted in `main` then
/// waits on whichever replica handles the next read. `await_applied` could only
/// ever wait on the replica that handled the *write*, which is the wrong one as
/// often as not — and it charged up to 2s for the privilege even when nothing read
/// afterwards.
pub struct UserService {
    pub users: UserRepository,
    pub js: Context,
}

impl UserService {
    /// Returns the log position, so the client can echo it and have the next call
    /// — a second submit of the same form, most usefully — read this write.
    pub async fn signup(&self, req: SignupRequest) -> MyResult<u64> {
        // Turns the common case into a 409. It is not the guard, though — see the
        // deterministic event id below for the concurrent case this read cannot
        // see, and `email_idx … UNIQUE` for the backstop behind both.
        //
        // A double-submitted form only gets the 409 if the first signup has been
        // projected before the second one reads. That is what the seq this returns
        // buys: the same client's second request carries `USERS:<seq>` and the
        // layer holds it until the row is there. Without the echo it falls through
        // to the dedupe window below, which is silent — the same account, answered
        // twice with success.
        self.users
            .find_by_email(req.email.to_string())
            .await?
            .is_none()
            .context_conflict((
                "Email already registered",
                "An account with that email already exists.",
            ))?;

        // Deterministic event id, so two signups racing on the same address
        // collapse into one append: the id rides `Nats-Msg-Id`, and the stream's
        // 120s `duplicate_window` discards the second server-side, atomically.
        //
        // This is the only guard that sees a *concurrent* duplicate — a
        // double-clicked button is enough. The read above cannot: both requests
        // run it before either publishes, both see None, and both would append a
        // `Registered` carrying a different `user_id`. The projector applies the
        // first, hits `email_idx … UNIQUE` on the second, and stops. That event is
        // in the log for good, so every replay stops at it too: not a failed
        // replica, a permanently unbuildable projection.
        //
        // `publish_expecting` cannot do this job. It is a compare-and-swap on one
        // *subject*, and each signup mints a fresh `user_id` — so the two racers
        // publish to different subjects and both satisfy `Some(0)`. Making CAS bite
        // would mean deriving the subject from the address, which both leaks
        // email→user_id to anyone holding a user id and makes an abandoned address
        // permanently unclaimable.
        //
        // Deliberately *not* normalised: the dedupe key must partition addresses
        // exactly as `email_idx` does, and that index is on the raw string. Lower
        // casing here would merge two signups the database considers distinct.
        //
        // Same pattern, and the same reason, as payment-service's event ids.
        //
        // Derived up here rather than beside the assignment below only because the
        // address is moved into the event in between.
        let event_id = Uuid::new_v5(
            &Uuid::NAMESPACE_OID,
            format!("signup:{}", req.email).as_bytes(),
        );

        let user_id = Uuid::now_v7();
        let shard = shard_of(&user_id);
        let event = UserEvent::Registered(UserRegistered {
            user_id,
            shard: shard.clone(),
            first_name: req.first_name,
            last_name: req.last_name,
            email: req.email.into(),
            // Hashed here, not in the projection: Argon2 salts randomly, so a
            // projection would produce a different hash on every replica.
            password_hash: password::hash(req.password.as_str())?,
        });

        let mut envelope = Envelope::new(event, Some(user_id));
        envelope.event_id = event_id;

        bus::publish(&self.js, user_subject(&shard, &user_id), &envelope).await
    }

    /// Checks credentials and returns the user they belong to.
    ///
    /// Mints nothing — `route::login` hands the result to `RefreshTokenService`.
    /// Keeping the two apart is what stops the session service from needing to
    /// know how a password is stored.
    pub async fn authenticate(&self, req: LoginRequest) -> MyResult<User> {
        let found = self.users.find_by_email(req.email.to_string()).await?;

        // Verify even when no user matched, against a throwaway hash, so a missing
        // account and a wrong password take the same time to answer.
        let ok = match &found {
            Some(u) => password::verify(&u.password, &req.password),
            None => {
                password::verify(
                    "$argon2id$v=19$m=19456,t=2,p=1$c2FsdHNhbHRzYWx0$0000000000000000000000000000000000000000000",
                    &req.password,
                );
                false
            }
        };
        // `filter` rather than a check plus an unwrap: a missing user and a wrong
        // password collapse into the same 401 from the same expression, so there is
        // no branch where one of them could grow a different answer.
        let user = found
            .filter(|_| ok)
            .context_unauthorized(("Unauthorized", "Invalid credentials"))?;

        // After the password check, never before. Answering "verify your email" to
        // an unauthenticated caller would confirm the address is registered,
        // turning the login form into an account-enumeration oracle — the whole
        // reason the branch above burns a hash on a missing user.
        //
        // Distinct title so the client can tell this apart from a wrong password
        // and offer to re-send instead of "check your credentials".
        user.email_verified.context_forbidden((
            "Email not verified",
            "Check your inbox for the verification link before logging in.",
        ))?;

        Ok(user)
    }

    /// Marks an address confirmed, given a token only that mailbox received.
    ///
    /// Unauthenticated on purpose: the token *is* the credential. It is verified
    /// under `EMAIL_TOKEN_SECRET` and `Purpose::VerifyEmail`, so it cannot be a
    /// repurposed access token and cannot be a password-reset link.
    ///
    /// **Deliberately idempotent.** Mail scanners prefetch links, and a user who
    /// clicks twice is not an error. Publishing `EmailVerified` a second time is a
    /// no-op in the projection, so there is nothing to guard against — and an
    /// "already used" check here would be the bug, not the fix.
    ///
    /// Mints no session. Proving an address is reachable and authenticating a
    /// person are different claims; a link that logs someone in is exactly what
    /// scanner prefetch turns into an account compromise.
    ///
    /// Returns the log position, which the client echoes on the login that follows
    /// — `authenticate` reads `email_verified`, so a login racing this projection
    /// would answer "verify your email" to someone who just did.
    pub async fn verify_email(&self, req: VerifyEmailRequest) -> MyResult<u64> {
        let user_id = shared::email_token::verify(
            &CONFIG.email_token_secret,
            &req.token,
            shared::email_token::Purpose::VerifyEmail,
        )?;

        // The token proves which account, but not which shard — and events for a
        // user must stay on the subject their history already lives on.
        let user = self
            .users
            .find_by_id(user_id)
            .await?
            .context_not_found(("Not Found", "Could not find user"))?;

        bus::publish(
            &self.js,
            user_subject(&user.shard, &user_id),
            &Envelope::new(UserEvent::EmailVerified { user_id }, Some(user_id)),
        )
        .await
    }

    /// Asks notification-service to send the verification link again.
    ///
    /// Returns `Ok(())` whether or not the address exists, and whether or not it is
    /// already verified. Anything else makes this an account-enumeration oracle for
    /// an endpoint that needs no credentials at all — the same reason `revoke`
    /// shrugs at an unknown token.
    ///
    // ponytail: no rate limit. One event per request, and the event is what costs
    // money to deliver. Add a per-user cooldown (last-sent timestamp on the row,
    // checked here) if this ever gets pointed at.
    pub async fn resend_verification(&self, req: ResendVerificationRequest) -> MyResult<()> {
        let Some(user) = self.users.find_by_email(req.email.to_string()).await? else {
            return Ok(());
        };

        if user.email_verified {
            return Ok(());
        }

        // No seq answered and none needed: `VerificationRequested` is projected by
        // nothing — it is a message to notification-service — so there is no state
        // here for a follow-up read to be waiting on.
        bus::publish(
            &self.js,
            user_subject(&user.shard, &user.id),
            &Envelope::new(
                UserEvent::VerificationRequested(VerificationRequested {
                    user_id: user.id,
                    email: user.email,
                    // Off the projection rather than the token: the template greets the
                    // reader by name and the token carries only an id. No second query
                    // for it — the row above is the whole user.
                    first_name: user.first_name,
                }),
                Some(user.id),
            ),
        )
        .await?;

        Ok(())
    }

    /// Writes the caller's own record — both forms that do so: the profile screen
    /// and the change-password screen. Returns the log position, which the caller
    /// answers with so the client can wait for the projection that serves its next
    /// read — view-service's for the profile, this service's own for a login.
    ///
    /// One request, one event, and **which** event is decided here: a
    /// `new_password` makes it `PasswordChanged`, anything else makes it `Updated`.
    /// The two stay separate events because only one of them may reach view-service
    /// — and because a consumer that wants to react to a password change (revoke
    /// sessions, mail the account) needs to match on it rather than sniff a field.
    ///
    /// Exactly one event is also why the two halves cannot arrive together: two
    /// publishes to one subject have no atomicity between them, so a request that
    /// failed on the second would leave the first in the log for good.
    ///
    /// No ownership lookup: the target is always the caller's own record.
    pub async fn update_user(&self, uid: &Uuid, req: UpdateUserRequest) -> MyResult<u64> {
        let existing = self
            .users
            .find_by_id(*uid)
            .await?
            .context_not_found(("Not Found", "Could not find user"))?;
        let user_uuid = existing.id;

        // Destructured rather than read through `req`, so a field added to
        // `UpdateUserRequest` stops compiling here — which is the one place that has
        // to decide which of the two halves it belongs to.
        let UpdateUserRequest {
            first_name,
            last_name,
            email,
            license_plates,
            current_password,
            new_password,
            profile_picture,
        } = req;

        let event = match new_password {
            Some(new_password) => {
                // Only one event goes out, so a profile field here would be accepted
                // and then silently dropped. 422 instead.
                let profile_too = first_name.is_some()
                    || last_name.is_some()
                    || email.is_some()
                    || license_plates.is_some()
                    || profile_picture.is_some();
                (!profile_too).context_unprocessable_entity((
                    "One change at a time",
                    "Send a password change on its own.",
                ))?;

                // Proves the caller owns the account rather than merely holding an
                // access token for it — the same reason an email change pays for it
                // below.
                let current = current_password.as_deref().context_unprocessable_entity((
                    "Password required",
                    "Enter your current password to set a new one.",
                ))?;
                password::verify(&existing.password, current)
                    .context_unauthorized(("Unauthorized", "Incorrect password"))?;

                UserEvent::PasswordChanged(UserPasswordChanged {
                    user_id: user_uuid,
                    password_hash: password::hash(new_password.as_str())?,
                })
            }

            None => {
                // Changing the address a password reset would be sent to is an
                // account takeover if it's left unguarded. Verification now catches
                // it afterwards too — the projector clears `email_verified` on any
                // address change, so the account is locked out of login until the
                // new one is confirmed — but this check stays: it stops the takeover
                // instead of merely stranding the victim's account behind a link
                // sent to the attacker. The rest of the form is harmless, so only
                // this branch pays for it.
                //
                // `None` is "unchanged", so it is not a change of address.
                if let Some(email) = email
                    .as_ref()
                    .map(Email::as_str)
                    .filter(|e| *e != existing.email)
                {
                    let current = current_password.as_deref().context_unprocessable_entity((
                        "Password required",
                        "Enter your current password to change your email address.",
                    ))?;

                    password::verify(&existing.password, current)
                        .context_unauthorized(("Unauthorized", "Incorrect password"))?;

                    // Same as signup: the unique index is the real guard, this only
                    // turns the common case into a 409 rather than a projector
                    // failure on an event that is already in the log.
                    self.users
                        .find_by_email(email.to_string())
                        .await?
                        .is_none()
                        .context_conflict((
                            "Email already registered",
                            "An account with that email already exists.",
                        ))?;
                }

                // Straight through, all of them: `None` already means "unchanged"
                // both in the event and in the projection's `?? column` coalescing,
                // which is the same thing it means in the request.
                UserEvent::Updated(UserUpdated {
                    user_id: user_uuid,
                    first_name,
                    last_name,
                    email: email.map(Into::into),
                    license_plates,
                    profile_picture,
                })
            }
        };

        bus::publish(
            &self.js,
            user_subject(&existing.shard, &user_uuid),
            &Envelope::new(event, Some(user_uuid)),
        )
        .await
    }
}
