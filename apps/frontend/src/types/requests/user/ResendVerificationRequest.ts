/**
 * `POST /api/user/email/resend`. `shared::requests::user::ResendVerificationRequest`.
 *
 * Always 204, even for an address with no account — the backend refuses to say which
 * addresses are registered, so there is nothing here to branch on.
 */
export type ResendVerificationRequest = {
  email: string;
};
