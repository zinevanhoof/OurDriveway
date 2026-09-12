/**
 * `POST /api/user/email/verify`. `shared::requests::user::VerifyEmailRequest`.
 *
 * A POST, not a GET, even though it is reached by clicking a link: mail scanners
 * prefetch links, so the link goes to a page and only this call has an effect. Safe to
 * run twice — re-verification is a no-op server-side, which is what makes a prefetch
 * harmless.
 */
export type VerifyEmailRequest = {
  token: string;
};
