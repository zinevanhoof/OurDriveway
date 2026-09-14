/**
 * `POST /api/user/password/forgot`. `shared::requests::user::ForgotPasswordRequest`.
 *
 * Always 204, even for an address with no account — the backend refuses to say which
 * addresses are registered, so there is nothing here to branch on. The screen has to
 * say the same thing either way.
 */
export type ForgotPasswordRequest = {
  email: string;
};
