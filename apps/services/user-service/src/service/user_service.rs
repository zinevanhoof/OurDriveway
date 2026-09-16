use bus::outbox;
use chrono::Utc;
use diesel_async::AsyncConnection;
use diesel_async::scoped_futures::ScopedFutureExt;
use shared::db;
use shared::domain_models::user::{User, UserPatch};
use shared::error::myerror::{ContextExt, MyError, MyResult};
use shared::events::user::{
    PasswordResetRequested, UserEvent, UserPasswordChanged, UserRegistered, UserUpdated,
    VerificationRequested,
};
use shared::events::{Envelope, aggregate_id, format_version, user_subject};
use shared::requests::user::{
    Country, Email, ForgotPasswordRequest, LoginRequest, ResendVerificationRequest,
    ResetPasswordRequest, SignupRequest, UpdateUserRequest, VerifyEmailRequest,
};
use uuid::Uuid;

use crate::{
    CONFIG,
    auth::password,
    repository::{
        refresh_token_repository::RefreshTokenRepository, user_repository::UserRepository,
    },
};

/// Write side for the user itself: who they are, and proving it.
///
/// Writes its own rows and enqueues the event beside them, in one transaction.
/// Reads go straight to the models in `shared::domain_models`.
///
/// Sessions are [`crate::service::refresh_token_service::RefreshTokenService`]'s.
/// This one stops at `authenticate`, which answers "are these credentials good"
/// and hands back the row; deciding what token to mint for it is the other
/// service's call.
/// Every write answers with `user:<id>@<version>`, which the client echoes as
/// `X-Await-Version`; the [`bus::await_version`] layer mounted in `main` holds a
/// following read until this service's own rows have reached it. Reading back what
/// this service just wrote needs no wait at all — the transaction committed.
pub struct UserService {
    /// The pool, not a repository. The repositories are stateless now — sqlx's
    /// `PgExecutor` covers both `&PgPool` and the `&mut PgConnection` inside an open
    /// transaction, so there is nothing for a repository to hold.
    pub db: shared::db::Db,
}

impl UserService {
    /// Returns `user:<id>@<version>`, so the client can echo it and have the next
    /// call — a second submit of the same form, most usefully — see this write.
    pub async fn signup(&self, req: SignupRequest) -> MyResult<String> {
        // Turns the common case into a 409. It is not the guard, though — see the
        // deterministic event id below for the concurrent case this read cannot
        // see, and `email_idx … UNIQUE` for the backstop behind both.
        //
        // A double-submitted form only gets the 409 if the first signup has been
        // projected before the second one reads. That is what the version this returns
        // buys: the same client's second request carries `user:<id>@<n>` and the
        // layer holds it until the row is there. Without the echo it falls through
        // to the dedupe window below, which is silent — the same account, answered
        // twice with success.
        let mut read = db::conn(&self.db).await?;
        UserRepository::find_by_email(&mut read, req.email.to_string())
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
        // Beside the event rather than in it. Argon2 salts randomly, so this still
        // has to happen exactly once and on the write side — but the result belongs
        // in the row and nowhere else, least of all in a stream four services read.
        let password_hash = password::hash(req.password.as_str())?;
        let registered = UserRegistered {
            user_id,
            first_name: req.first_name,
            last_name: req.last_name,
            email: req.email.into(),
        };

        // The row and its event, in one transaction. This is the whole shape of
        // the rewrite: the database is authoritative, and the event is a durable
        // side effect of the same commit rather than the thing that caused it.
        let mut conn = db::conn(&self.db).await?;

        let version = conn
            .transaction::<_, MyError, _>(|conn| {
                async move {
                    let version =
                        shared::next_version!(conn, shared::schema::user::app_user, &user_id)?;

                    // No `set_version` after this: the row carries its own version and this is
                    // a whole-row write. The separate statement is still needed wherever a
                    // *patch* moves a row, since a patch does not touch the column.
                    UserRepository::upsert(
                        conn,
                        User::registered(registered.clone(), version, password_hash),
                    )
                    .await?;

                    let mut envelope = Envelope::new(
                        UserEvent::Registered(registered),
                        Some(user_id),
                        aggregate_id("user", &user_id),
                        version,
                    );
                    envelope.event_id = event_id;

                    outbox::enqueue(conn, &user_subject(&user_id), &envelope).await?;
                    Ok(format_version(&envelope.aggregate, envelope.version))
                }
                .scope_boxed()
            })
            .await?;

        Ok(version)
    }

    /// Checks credentials and returns the user they belong to.
    ///
    /// Mints nothing — `route::login` hands the result to `RefreshTokenService`.
    /// Keeping the two apart is what stops the session service from needing to
    /// know how a password is stored.
    pub async fn authenticate(&self, req: LoginRequest) -> MyResult<User> {
        let mut read = db::conn(&self.db).await?;
        let found = UserRepository::find_by_email(&mut read, req.email.to_string()).await?;

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
    pub async fn verify_email(&self, req: VerifyEmailRequest) -> MyResult<String> {
        // `.user_id` and nothing else: a verification token carries no version claim
        // and binds to no state — this endpoint is replayable by design.
        let user_id = shared::email_token::verify(
            &CONFIG.email_token_secret,
            &req.token,
            shared::email_token::Purpose::VerifyEmail,
        )?
        .user_id;

        let mut conn = db::conn(&self.db).await?;

        let version = conn
            .transaction::<_, MyError, _>(|conn| {
                async move {
                    // Read inside the transaction: the token proves which account, but this
                    // must refuse one naming a user who no longer exists rather than writing
                    // for them.
                    UserRepository::find_by_id(conn, user_id)
                        .await?
                        .context_not_found(("Not Found", "Could not find user"))?;

                    // Takes `FOR UPDATE` on the row, which is what serialises two of these
                    // against each other now that a contended write no longer conflicts on its
                    // own. See `shared::db::next_version`.
                    let version =
                        shared::next_version!(conn, shared::schema::user::app_user, &user_id)?;
                    // Idempotent by construction — setting `true` twice is setting `true`.
                    // That matters because mail scanners prefetch links, so this endpoint is
                    // deliberately re-runnable.
                    UserRepository::patch(
                        conn,
                        user_id,
                        UserPatch {
                            email_verified: Some(true),
                            ..Default::default()
                        },
                    )
                    .await?;
                    shared::set_version!(
                        conn,
                        "user",
                        shared::schema::user::app_user,
                        &user_id,
                        version
                    )?;

                    let envelope = Envelope::new(
                        UserEvent::EmailVerified { user_id },
                        Some(user_id),
                        aggregate_id("user", &user_id),
                        version,
                    );

                    outbox::enqueue(conn, &user_subject(&user_id), &envelope).await?;
                    Ok(format_version(&envelope.aggregate, envelope.version))
                }
                .scope_boxed()
            })
            .await?;

        Ok(version)
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
        let mut read = db::conn(&self.db).await?;
        let Some(user) = UserRepository::find_by_email(&mut read, req.email.to_string()).await?
        else {
            return Ok(());
        };

        if user.email_verified {
            return Ok(());
        }

        // No version answered and none needed: `VerificationRequested` is projected by
        // nothing — it is a message to notification-service — so there is no state
        // here for a follow-up read to be waiting on.
        // A transaction for an event that writes no row, which looks odd until you
        // ask where else the outbox row would go: the enqueue *is* the write, and it
        // still has to be atomic with the version it claims.
        let mut conn = db::conn(&self.db).await?;

        conn.transaction::<_, MyError, _>(|conn| {
            async move {
                let version =
                    shared::next_version!(conn, shared::schema::user::app_user, &user.id)?;
                shared::set_version!(
                    conn,
                    "user",
                    shared::schema::user::app_user,
                    &user.id,
                    version
                )?;

                let envelope = Envelope::new(
                    UserEvent::VerificationRequested(VerificationRequested {
                        user_id: user.id,
                        email: user.email,
                        // Off the projection rather than the token: the template greets the
                        // reader by name and the token carries only an id. No second query
                        // for it — the row above is the whole user.
                        first_name: user.first_name,
                    }),
                    Some(user.id),
                    aggregate_id("user", &user.id),
                    version,
                );

                outbox::enqueue(conn, &user_subject(&user.id), &envelope).await?;
                Ok(())
            }
            .scope_boxed()
        })
        .await?;

        Ok(())
    }

    /// Asks notification-service to mail a password-reset link.
    ///
    /// Returns `Ok(())` whether or not the address exists — the same enumeration
    /// rule as `resend_verification`, and for the same reason: this needs no
    /// credentials at all, so any answer that varies is an oracle telling an
    /// anonymous caller which addresses are registered.
    ///
    /// Unverified accounts are included deliberately. A reset link proves the
    /// mailbox received it, which is the very claim verification makes, so
    /// `reset_password` marks the address verified rather than leaving someone
    /// who reset successfully still unable to log in and no way to find out why.
    ///
    /// **The version this bumps to is the mechanism, not bookkeeping.**
    /// notification-service mints the token carrying it and `reset_password`
    /// refuses that token unless the row is still there, which is what makes the
    /// link single-use with nothing stored anywhere. `set_version!` is therefore
    /// mandatory here even though no row changes: without it the row never
    /// reaches the version the token names and every link is born dead.
    ///
    // ponytail: no rate limit, same as `resend_verification` — but this is the
    // endpoint that actually gets pointed at, and each request is a mail to a real
    // inbox that reads as phishing when it wasn't asked for. Add a per-user
    // cooldown (last-sent timestamp on the row, checked here) before this is
    // public. Note the frontend hides its own button after one send, which covers
    // the double-click but nothing deliberate.
    pub async fn forgot_password(&self, req: ForgotPasswordRequest) -> MyResult<()> {
        let mut read = db::conn(&self.db).await?;
        let Some(user) = UserRepository::find_by_email(&mut read, req.email.to_string()).await?
        else {
            return Ok(());
        };

        // No version answered and none needed, exactly as `resend_verification`:
        // `PasswordResetRequested` is projected by nothing — it is a message to
        // notification-service — so there is no state for a follow-up read to wait
        // on. The transaction is still required: the enqueue *is* the write, and it
        // has to be atomic with the version it claims.
        let mut conn = db::conn(&self.db).await?;

        conn.transaction::<_, MyError, _>(|conn| {
            async move {
                let version =
                    shared::next_version!(conn, shared::schema::user::app_user, &user.id)?;
                shared::set_version!(
                    conn,
                    "user",
                    shared::schema::user::app_user,
                    &user.id,
                    version
                )?;

                let envelope = Envelope::new(
                    UserEvent::PasswordResetRequested(PasswordResetRequested {
                        user_id: user.id,
                        email: user.email,
                        first_name: user.first_name,
                    }),
                    Some(user.id),
                    aggregate_id("user", &user.id),
                    version,
                );

                outbox::enqueue(conn, &user_subject(&user.id), &envelope).await?;
                Ok(())
            }
            .scope_boxed()
        })
        .await?;

        Ok(())
    }

    /// Sets a new password from a mailed link, and ends every session on the
    /// account.
    ///
    /// Unauthenticated on purpose: the token *is* the credential, verified under
    /// `EMAIL_TOKEN_SECRET` and `Purpose::ResetPassword` so it can be neither a
    /// repurposed access token nor a verification link.
    ///
    /// **Single use, with nothing stored to make it so.** The token carries the
    /// aggregate version the account stood at when `forgot_password` raised its
    /// event; this bumps the row past it. A second click reads a version one
    /// higher than the token names and gets the identical 400 a forged link does —
    /// which is why every rejection below is `email_token::invalid()` rather than
    /// a message of its own. This is the opposite of `verify_email`, which is
    /// deliberately replayable; setting `true` twice is harmless, setting a
    /// password twice is a replay hole.
    ///
    /// The refresh tokens die inside the same transaction as the password: whoever
    /// forced the reset must not keep a session across it. That does **not** reach
    /// an access token already issued — a JWT is stateless and stays good until it
    /// expires. Short `JWT_EXPIRATION` is the whole mitigation for that window.
    ///
    /// Returns the log position, which the client echoes on the login that follows
    /// — `authenticate` reads the password and `email_verified` from this
    /// service's own rows.
    pub async fn reset_password(&self, req: ResetPasswordRequest) -> MyResult<String> {
        let verified = shared::email_token::verify(
            &CONFIG.email_token_secret,
            &req.token,
            shared::email_token::Purpose::ResetPassword,
        )?;
        let user_id = verified.user_id;
        // A `Result`, not an `Option`: a token carrying no version claim is refused
        // here rather than reaching the comparison below as an unchecked `None`.
        let minted_at = verified.version()?;

        // Hashed before the transaction opens. Argon2 is ~100ms of CPU by design and
        // `next_version!` holds `FOR UPDATE` on the row for the whole block —
        // hashing inside would serialise every other write to this user behind it.
        let password_hash = password::hash(req.password.as_str())?;

        let mut conn = db::conn(&self.db).await?;

        let version = conn
            .transaction::<_, MyError, _>(|conn| {
                async move {
                    // Takes `FOR UPDATE`, which is what serialises two clicks of the same
                    // link against each other: the second reads the version the first wrote.
                    let next =
                        shared::next_version!(conn, shared::schema::user::app_user, &user_id)?;

                    // The single-use check. `next_version!` returns stored + 1, so this
                    // reads "the row is still exactly where it was when the link was minted".
                    //
                    // No `find_by_id` above it, unlike `verify_email`: a row that is gone
                    // makes `next` 1, which would need `minted_at == 0`, and versions start
                    // at 1. A deleted user therefore falls out here as an invalid link —
                    // which is the better answer anyway, since a 404 would confirm to an
                    // anonymous caller that the id once existed.
                    if next - 1 != minted_at {
                        return Err(shared::email_token::invalid());
                    }

                    UserRepository::patch(
                        conn,
                        user_id,
                        UserPatch {
                            password: Some(password_hash),
                            // The link proved the mailbox, which is the same claim
                            // `EmailVerified` makes. Without this an account that never
                            // verified resets successfully and still cannot log in.
                            email_verified: Some(true),
                            ..Default::default()
                        },
                    )
                    .await?;
                    shared::set_version!(
                        conn,
                        "user",
                        shared::schema::user::app_user,
                        &user_id,
                        next
                    )?;

                    // In this transaction rather than after it: the password and the
                    // sessions it protected die together, or neither does.
                    let ended = RefreshTokenRepository::revoke_all_for_user(
                        conn,
                        user_id,
                        "password reset",
                    )
                    .await?;
                    tracing::info!(%user_id, sessions_ended = ended, "password reset");

                    // `PasswordChanged`, not a variant of its own: view-service and
                    // payment-service already ignore it, and a consumer that reacts to a
                    // password changing does not care why it changed — nor, now, what it
                    // changed to.
                    let envelope = Envelope::new(
                        UserEvent::PasswordChanged(UserPasswordChanged { user_id }),
                        Some(user_id),
                        aggregate_id("user", &user_id),
                        next,
                    );

                    outbox::enqueue(conn, &user_subject(&user_id), &envelope).await?;
                    Ok(format_version(&envelope.aggregate, envelope.version))
                }
                .scope_boxed()
            })
            .await?;

        Ok(version)
    }

    /// Writes the caller's own record — both forms that do so: the edit screen
    /// and the change-password screen. Returns the log position, which the caller
    /// answers with so the client can wait for the projection that serves its next
    /// read — view-service's for the user row, this service's own for a login.
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
    pub async fn update_user(&self, uid: &Uuid, req: UpdateUserRequest) -> MyResult<String> {
        let mut read = db::conn(&self.db).await?;
        let existing = UserRepository::find_by_id(&mut read, *uid)
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
            country,
            current_password,
            new_password,
            profile_picture,
        } = req;

        // The hash travels *beside* the event now rather than inside it — see
        // `UserRegistered`. Destructured as a pair so the invariant holds by
        // construction: a `PasswordChanged` always carries one, an `Updated` never
        // does, and neither arm can be written to disagree.
        let (event, new_password_hash) = match new_password {
            Some(new_password) => {
                // Only one event goes out, so any other field here would be accepted
                // and then silently dropped. 422 instead.
                let user_fields_too = first_name.is_some()
                    || last_name.is_some()
                    || email.is_some()
                    || license_plates.is_some()
                    || country.is_some()
                    || profile_picture.is_some();
                (!user_fields_too).context_conflict((
                    "One change at a time",
                    "Send a password change on its own.",
                ))?;

                // Proves the caller owns the account rather than merely holding an
                // access token for it — the same reason an email change pays for it
                // below.
                let current = current_password.as_deref().context_conflict((
                    "Password required",
                    "Enter your current password to set a new one.",
                ))?;
                password::verify(&existing.password, current)
                    .context_unauthorized(("Unauthorized", "Incorrect password"))?;

                (
                    UserEvent::PasswordChanged(UserPasswordChanged { user_id: user_uuid }),
                    Some(password::hash(new_password.as_str())?),
                )
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
                    let current = current_password.as_deref().context_conflict((
                        "Password required",
                        "Enter your current password to change your email address.",
                    ))?;

                    password::verify(&existing.password, current)
                        .context_unauthorized(("Unauthorized", "Incorrect password"))?;

                    // Same as signup: the unique index is the real guard, this only
                    // turns the common case into a 409 rather than a projector
                    // failure on an event that is already in the log.
                    let mut check = db::conn(&self.db).await?;
                    UserRepository::find_by_email(&mut check, email.to_string())
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
                (
                    UserEvent::Updated(UserUpdated {
                        user_id: user_uuid,
                        first_name,
                        last_name,
                        email: email.map(Into::into),
                        license_plates,
                        profile_picture,
                        // Uppercased on the way out — `Country::into_inner` is the only
                        // way to read one, so the database and every consumer of this
                        // event see a single spelling.
                        country: country.map(Country::into_inner),
                    }),
                    None,
                )
            }
        };

        let mut conn = db::conn(&self.db).await?;

        let version = conn
            .transaction::<_, MyError, _>(|conn| {
                async move {
                    let version =
                        shared::next_version!(conn, shared::schema::user::app_user, &user_uuid)?;

                    match &event {
                        // The hash comes from the pair above, not from the event —
                        // which no longer carries one. `PasswordChanged` implies
                        // `Some`, so an unreachable `None` writes nothing rather than
                        // panicking: this is inside a transaction, and the version
                        // bump and the event below are still correct on their own.
                        UserEvent::PasswordChanged(_) => {
                            UserRepository::patch(
                                conn,
                                user_uuid,
                                UserPatch {
                                    password: new_password_hash.clone(),
                                    ..Default::default()
                                },
                            )
                            .await?;
                        }

                        // Order is the point, and it used to be the projector's: the submitted
                        // address has to be compared against the stored one *before* it is
                        // overwritten. Without this, changing to an unverified address keeps the
                        // flag from the old one and login lets it straight through, which makes
                        // the whole feature decorative.
                        UserEvent::Updated(e) => {
                            if e.email.as_ref().is_some_and(|new| *new != existing.email) {
                                UserRepository::patch(
                                    conn,
                                    user_uuid,
                                    UserPatch {
                                        email_verified: Some(false),
                                        ..Default::default()
                                    },
                                )
                                .await?;
                            }
                            UserRepository::patch(conn, user_uuid, e.clone().into()).await?;
                        }

                        // `update_user` builds only the two variants above.
                        _ => {}
                    }

                    shared::set_version!(
                        conn,
                        "user",
                        shared::schema::user::app_user,
                        &user_uuid,
                        version
                    )?;

                    let envelope = Envelope::new(
                        event,
                        Some(user_uuid),
                        aggregate_id("user", &user_uuid),
                        version,
                    );

                    outbox::enqueue(conn, &user_subject(&user_uuid), &envelope).await?;
                    Ok(format_version(&envelope.aggregate, envelope.version))
                }
                .scope_boxed()
            })
            .await?;

        Ok(version)
    }

    /// Re-emits every user as the events that reproduce their current row, for a
    /// consumer that needs rebuilding. See [`outbox::backfill`] for what this is and
    /// is not.
    ///
    /// Two events each, and both are needed. `Registered` is the only variant
    /// view-service will create a row from, and it deliberately lands with no
    /// picture and no plates because a fresh signup has neither — so `Updated` puts
    /// back whatever the account has changed since.
    ///
    /// `EmailVerified` is not among them and is not an omission: nothing downstream
    /// projects it. Verification stays in this service's own row, which is the thing
    /// being read here rather than rebuilt. `VerificationRequested` and
    /// `PasswordResetRequested` are left out for a much louder reason — both are a
    /// mail, and re-emitting either would send one to every account on the system.
    /// A reset mail would additionally hand every one of them a live credential.
    /// `Envelope::backfill` is the real backstop, checked in
    /// notification-service's `notify`; this list is what stops anyone reaching
    /// for it in the first place.
    ///
    /// `PasswordChanged` is likewise absent, and now trivially so: it carries no
    /// password, so there is nothing about it to reproduce. This used to publish
    /// every account's Argon2 hash onto STREAM_USERS in one burst — the single worst
    /// consequence of a field nothing downstream ever read.
    pub async fn backfill(&self) -> MyResult<usize> {
        let mut sent = 0;

        let mut read = db::conn(&self.db).await?;
        for user in UserRepository::all(&mut read).await? {
            let user_id = user.id;

            let registered = UserRegistered {
                user_id,
                first_name: user.first_name,
                last_name: user.last_name,
                email: user.email,
            };
            let updated = UserUpdated {
                user_id,
                // The three `Registered` above already carries. `None` is "leave
                // alone", so re-stating them would only be a chance to disagree.
                first_name: None,
                last_name: None,
                email: None,
                profile_picture: user.profile_picture,
                license_plates: Some(user.license_plates),
                country: user.country,
            };

            // This table keeps no timestamp of its own and nothing downstream stores
            // one off a USERS event, so there is no original clock to recover here —
            // unlike spots and bookings, whose rows carry theirs.
            let now = Utc::now();

            sent += outbox::backfill(
                &self.db,
                &user_subject(&user_id),
                &aggregate_id("user", &user_id),
                user.version,
                [
                    (now, UserEvent::Registered(registered)),
                    (now, UserEvent::Updated(updated)),
                ],
            )
            .await?;
        }

        tracing::info!(events = sent, "users backfilled");
        Ok(sent)
    }
}
