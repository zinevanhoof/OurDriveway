/**
 * `POST /api/user/password/reset`. `shared::requests::user::ResetPasswordRequest`.
 *
 * The opposite of `VerifyEmailRequest`, which is deliberately replayable: this token
 * works exactly once. Sending it a second time — or sending an older link after a
 * newer one was requested — gets the same 400 a forged token does, because a used
 * link and a fake one must not be distinguishable. Treat that status as "ask for a
 * new link", never as "try again".
 *
 * No current password: the token is the credential.
 */
export type ResetPasswordRequest = {
  token: string;
  password: string;
};
